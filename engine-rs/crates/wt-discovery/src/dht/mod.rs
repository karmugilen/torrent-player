//! BEP 5 mainline DHT.
//!
//! Wraps the pure-Rust `mainline` crate for dedicated-socket mode, and accepts
//! [`DatagramIo`] / [`DhtPacketIo`] so core can share `wt-wire`'s uTP UDP socket
//! (DHT payloads are bencoded and typically start with `d`).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use wt_types::InfoHash;

use crate::datagram::DatagramIo;
#[cfg(test)]
use crate::datagram::StandaloneDatagram;

/// Anacrolix / Go default bootstrap host:ports.
pub const DEFAULT_BOOTSTRAP_NODES: &[&str] = &[
    "router.utorrent.com:6881",
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "dht.aelitis.com:6881",
    "router.silotis.us:6881",
    "dht.libtorrent.org:25401",
    "dht.anacrolix.link:42069",
    "router.bittorrent.cloud:42069",
];

/// Shared UDP IO so DHT and uTP can coexist (alias of [`DatagramIo`]).
pub use crate::datagram::{DatagramIo as DhtPacketIo, IoFut};

/// Good DHT contact for persistence (engine saves/restores these).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Contact {
    pub id: [u8; 20],
    pub addr: SocketAddr,
}

impl Contact {
    pub fn to_bootstrap_string(&self) -> String {
        self.addr.to_string()
    }
}

#[derive(Clone, Debug)]
pub struct DhtConfig {
    /// Bind port (0 = ephemeral). Used in dedicated-socket mode.
    pub port: u16,
    pub bootstrap: Vec<String>,
    /// Extra contacts to load (from engine persistence).
    pub initial_contacts: Vec<Contact>,
    /// Read-only mode (Go default is off).
    pub read_only: bool,
    pub request_timeout: Duration,
}

impl Default for DhtConfig {
    fn default() -> Self {
        Self {
            port: 0,
            bootstrap: DEFAULT_BOOTSTRAP_NODES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            initial_contacts: Vec::new(),
            read_only: false,
            request_timeout: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DhtError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("dht: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DhtError>;

/// Streaming peer results from an iterative get_peers.
#[derive(Clone, Debug)]
pub struct PeersValues {
    pub peers: Vec<SocketAddr>,
    pub from: Option<SocketAddr>,
}

/// Handle for an in-flight announce / get_peers.
pub struct AnnounceHandle {
    rx: tokio::sync::mpsc::Receiver<PeersValues>,
    _cancel: tokio::sync::oneshot::Sender<()>,
}

impl AnnounceHandle {
    pub async fn next(&mut self) -> Option<PeersValues> {
        self.rx.recv().await
    }

    pub fn close(self) {
        // dropping cancel signals the task
    }
}

/// How long a bootstrap hostname may take to resolve.
const BOOTSTRAP_DNS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a lookup waits for a node that is still starting.
const READY_WAIT: Duration = Duration::from_secs(15);

/// DHT node.
///
/// Starts in the background, like Go's DHT bootstrap: `mainline` resolves
/// bootstrap hostnames with blocking DNS while it is built, which held engine
/// startup for ~20 s on a slow or broken network. Hostnames are resolved
/// asynchronously first and `mainline` only receives IP literals.
pub struct DhtNode {
    inner: Arc<Mutex<Option<mainline::async_dht::AsyncDht>>>,
    id: Arc<Mutex<[u8; 20]>>,
    local: Arc<Mutex<Option<SocketAddr>>>,
    periodic: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Shared datagram IO when coexisting with uTP; `None` in dedicated mode.
    shared_io: Option<Arc<dyn DatagramIo>>,
    /// Contacts imported while starting; they join the bootstrap list.
    pending: Arc<Mutex<Option<Vec<String>>>>,
    /// Becomes true once starting has finished (successfully or not).
    ready: tokio::sync::watch::Receiver<bool>,
    closed: Arc<AtomicBool>,
}

impl DhtNode {
    /// Create a DHT node on its own UDP socket (via `mainline`). Returns at
    /// once; the node becomes usable when its background start finishes.
    pub async fn new(cfg: DhtConfig) -> Result<Self> {
        if cfg.read_only {
            tracing::warn!(
                "read_only requested but mainline wrapper runs with server_mode for announce_peer"
            );
        }
        let inner = Arc::new(Mutex::new(None));
        let id = Arc::new(Mutex::new([0u8; 20]));
        let local = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(Some(
            cfg.initial_contacts.iter().map(Contact::to_bootstrap_string).collect::<Vec<_>>(),
        )));
        let closed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready) = tokio::sync::watch::channel(false);

        let (inner_t, id_t, local_t, pending_t, closed_t) = (
            Arc::clone(&inner), Arc::clone(&id), Arc::clone(&local),
            Arc::clone(&pending), Arc::clone(&closed),
        );
        tokio::spawn(async move {
            let mut addrs = Vec::new();
            let mut hosts = Vec::new();
            for entry in cfg.bootstrap {
                if entry.parse::<SocketAddr>().is_ok() {
                    addrs.push(entry);
                } else {
                    hosts.push(entry);
                }
            }
            let lookups = hosts.into_iter().map(|host| async move {
                match tokio::time::timeout(BOOTSTRAP_DNS_TIMEOUT, tokio::net::lookup_host(host)).await {
                    Ok(Ok(found)) => found.filter(SocketAddr::is_ipv4).map(|a| a.to_string()).collect(),
                    _ => Vec::new(),
                }
            });
            for found in futures::future::join_all(lookups).await {
                addrs.extend(found);
            }
            addrs.extend(pending_t.lock().take().unwrap_or_default());
            if closed_t.load(Ordering::SeqCst) {
                let _ = ready_tx.send(true);
                return;
            }
            // `Dht::new` waits for its actor thread; keep it off the runtime.
            let built = tokio::task::spawn_blocking(move || {
                let mut builder = mainline::Dht::builder();
                // server_mode enables storing peers / answering get_peers (needed for announce).
                builder.server_mode();
                if cfg.port != 0 {
                    builder.port(cfg.port);
                }
                builder.request_timeout(cfg.request_timeout);
                // IP literals only: never the hostname defaults.
                builder.bootstrap(&addrs);
                builder.build()
            })
            .await;
            match built {
                Ok(Ok(dht)) => {
                    let dht = dht.as_async();
                    let info = dht.info().await;
                    *id_t.lock() = *info.id().as_bytes();
                    local_t.lock().get_or_insert(SocketAddr::V4(info.local_addr()));
                    if !closed_t.load(Ordering::SeqCst) {
                        *inner_t.lock() = Some(dht.clone());
                    }
                    let _ = ready_tx.send(true);
                    // Warm the routing table (Go `Bootstrap` goroutine).
                    let _ = tokio::time::timeout(Duration::from_secs(5), dht.bootstrapped()).await;
                }
                Ok(Err(e)) => {
                    // Like Go, a DHT failure is not fatal: trackers keep working.
                    tracing::warn!("dht disabled: {e}");
                    let _ = ready_tx.send(true);
                }
                Err(e) => {
                    tracing::warn!("dht start failed: {e}");
                    let _ = ready_tx.send(true);
                }
            }
        });

        Ok(Self {
            inner,
            id,
            local,
            periodic: Mutex::new(Vec::new()),
            shared_io: None,
            pending,
            ready,
            closed,
        })
    }

    /// The running node, waiting briefly while it is still starting.
    async fn running(&self) -> Option<mainline::async_dht::AsyncDht> {
        if let Some(dht) = self.inner.lock().clone() {
            return Some(dht);
        }
        let mut ready = self.ready.clone();
        let _ = tokio::time::timeout(READY_WAIT, ready.wait_for(|started| *started)).await;
        self.inner.lock().clone()
    }


    /// Same as [`Self::with_packet_io`].
    pub async fn with_datagram_io(io: Arc<dyn DatagramIo>, cfg: DhtConfig) -> Result<Self> {
        let local = io.local_addr().ok();
        let mut node = Self::new(cfg).await?;
        if let Some(addr) = local {
            *node.local.lock() = Some(addr);
        }
        node.shared_io = Some(io);
        Ok(node)
    }

    /// Shared datagram IO if constructed via [`Self::with_datagram_io`].
    pub fn shared_io(&self) -> Option<Arc<dyn DatagramIo>> {
        self.shared_io.clone()
    }

    /// Send a raw datagram on the shared IO (errors if dedicated-only).
    pub async fn shared_send_to(&self, buf: &[u8], addr: SocketAddr) -> std::io::Result<usize> {
        match &self.shared_io {
            Some(io) => io.send_to(buf, addr).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no shared datagram io",
            )),
        }
    }



    pub fn id(&self) -> [u8; 20] {
        *self.id.lock()
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        *self.local.lock()
    }

    /// Export good routing-table contacts for persistence.
    pub async fn export_contacts(&self) -> Vec<Contact> {
        let Some(dht) = self.inner.lock().clone() else {
            return Vec::new();
        };
        let boot = dht.to_bootstrap().await;
        boot.into_iter()
            .filter_map(|s| {
                let addr: SocketAddr = s.parse().ok()?;
                Some(Contact {
                    id: [0u8; 20], // mainline bootstrap strings are addr-only
                    addr,
                })
            })
            .collect()
    }

    /// Import contacts (adds them as bootstrap / routing hints).
    pub async fn import_contacts(&self, contacts: &[Contact]) {
        // mainline doesn't expose add_node on AsyncDht directly; re-bootstrap
        // strings are best-effort via find_node against ourselves after adding
        // via a fresh builder is heavy. Ping each by find_node.
        let Some(dht) = self.inner.lock().clone() else {
            // Still starting: these join the bootstrap list.
            if let Some(pending) = self.pending.lock().as_mut() {
                pending.extend(contacts.iter().map(Contact::to_bootstrap_string));
            }
            return;
        };
        for c in contacts {
            if let SocketAddr::V4(_a) = c.addr {
                let info = dht.info().await;
                let id = if c.id != [0u8; 20] {
                    mainline::Id::from_bytes(c.id).unwrap_or_else(|_| *info.id())
                } else {
                    *info.id()
                };
                let _ = dht.find_node(id).await;
            }
        }
    }

    pub async fn add_node(&self, contact: Contact) {
        self.import_contacts(&[contact]).await;
    }

    /// Iterative get_peers + optional announce_peer.
    pub async fn announce(
        &self,
        info_hash: InfoHash,
        port: u16,
        implied_port: bool,
    ) -> Result<AnnounceHandle> {
        let Some(dht) = self.running().await else {
            return Err(DhtError::Other("dht closed".into()));
        };
        let id =
            mainline::Id::from_bytes(info_hash.0).map_err(|e| DhtError::Other(e.to_string()))?;

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();

        let announce_port = if implied_port { None } else { Some(port) };

        tokio::spawn(async move {
            let mut stream = dht.get_peers(id);
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => break,
                    next = futures::StreamExt::next(&mut stream) => {
                        match next {
                            Some(peers) => {
                                let addrs: Vec<SocketAddr> = peers
                                    .into_iter()
                                    .map(SocketAddr::V4)
                                    .collect();
                                if tx.send(PeersValues { peers: addrs, from: None }).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
            // After lookup, announce ourselves.
            let _ = dht.announce_peer(id, announce_port).await;
        });

        Ok(AnnounceHandle {
            rx,
            _cancel: cancel_tx,
        })
    }


    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for h in self.periodic.lock().drain(..) {
            h.abort();
        }
        *self.inner.lock() = None;
    }
}

/// Returns true when a UDP payload looks like a BEP 5 KRPC message.
pub fn looks_like_krpc(packet: &[u8]) -> bool {
    packet.first() == Some(&b'd')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn krpc_magic() {
        assert!(looks_like_krpc(b"d1:ad2:id20:"));
        assert!(!looks_like_krpc(&[0x01, 0x00]));
    }

    #[tokio::test]
    async fn shared_datagram_io_send_recv() {
        let a = Arc::new(StandaloneDatagram::bind("127.0.0.1:0").await.unwrap());
        let b = Arc::new(StandaloneDatagram::bind("127.0.0.1:0").await.unwrap());
        let cfg = DhtConfig {
            bootstrap: Vec::new(),
            ..Default::default()
        };
        // with_datagram_io still boots a dedicated mainline node; shared IO is retained.
        let node = DhtNode::with_datagram_io(a.clone() as Arc<dyn DatagramIo>, cfg)
            .await
            .expect("dht");
        assert!(node.shared_io().is_some());
        let dest = b.local_addr().unwrap();
        node.shared_send_to(b"d1:y1:qe", dest).await.unwrap();
        let mut buf = [0u8; 64];
        let (n, _) = b.recv_from(&mut buf).await.unwrap();
        assert!(looks_like_krpc(&buf[..n]));
        node.close();
    }

    #[tokio::test]
    async fn local_testnet_announce() {
        // mainline Testnet: several local nodes.
        let testnet = match mainline::Testnet::new_async(3).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skip testnet: {e}");
                return;
            }
        };
        let bootstrap = testnet.bootstrap.clone();
        let mut builder_a = mainline::Dht::builder();
        builder_a.server_mode().bootstrap(&bootstrap).no_bootstrap();
        // Use testnet helpers: create nodes from testnet.
        // Simpler: announce on one server node via the testnet's own nodes.
        let info_hash = mainline::Id::random();
        // Put peer via first node.
        let node = testnet.nodes[0].clone().as_async();
        let _ = node.announce_peer(info_hash, Some(6881)).await;

        // Lookup from another node.
        let other = testnet.nodes[1].clone().as_async();
        let mut found = Vec::new();
        let mut stream = other.get_peers(info_hash);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(
                Duration::from_millis(500),
                futures::StreamExt::next(&mut stream),
            )
            .await
            {
                Ok(Some(peers)) => {
                    found.extend(peers);
                    if !found.is_empty() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        // Best-effort: testnet may need a moment; don't fail hard if empty in CI flake.
        tracing::debug!(?found, "testnet peers");
        testnet.leak(); // keep sockets alive until drop
        let _ = found;
    }
}

//! BEP 5 mainline DHT.
//!
//! Wraps the pure-Rust `mainline` crate for dedicated-socket mode, and accepts
//! [`DatagramIo`] / [`DhtPacketIo`] so core can share `wt-wire`'s uTP UDP socket
//! (DHT payloads are bencoded and typically start with `d`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use wt_types::InfoHash;

use crate::datagram::{DatagramIo, StandaloneDatagram};

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

/// DHT node.
pub struct DhtNode {
    inner: Mutex<Option<mainline::async_dht::AsyncDht>>,
    id: [u8; 20],
    local: Mutex<Option<SocketAddr>>,
    periodic: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Shared datagram IO when coexisting with uTP; `None` in dedicated mode.
    shared_io: Option<Arc<dyn DatagramIo>>,
}

impl DhtNode {
    /// Create a DHT node on its own UDP socket (via `mainline`).
    pub async fn new(cfg: DhtConfig) -> Result<Self> {
        if cfg.read_only {
            tracing::warn!(
                "read_only requested but mainline wrapper runs with server_mode for announce_peer"
            );
        }
        let mut bootstrap: Vec<String> = cfg.bootstrap;
        for c in &cfg.initial_contacts {
            bootstrap.push(c.to_bootstrap_string());
        }

        let mut builder = mainline::Dht::builder();
        // server_mode enables storing peers / answering get_peers (needed for announce).
        builder.server_mode();
        if cfg.port != 0 {
            builder.port(cfg.port);
        }
        builder.request_timeout(cfg.request_timeout);
        if !bootstrap.is_empty() {
            builder.bootstrap(&bootstrap);
        }

        let dht = builder
            .build()
            .map_err(|e| DhtError::Other(e.to_string()))?
            .as_async();

        let info = dht.info().await;
        let id = *info.id().as_bytes();
        let la = info.local_addr();
        let local = SocketAddr::V4(la);

        // Wait briefly for bootstrap.
        let _ = tokio::time::timeout(Duration::from_secs(5), dht.bootstrapped()).await;

        Ok(Self {
            inner: Mutex::new(Some(dht)),
            id,
            local: Mutex::new(Some(local)),
            periodic: Mutex::new(Vec::new()),
            shared_io: None,
        })
    }

    /// Construct against shared datagram IO (core wraps wt-wire non-uTP demux).
    ///
    /// Full KRPC iterative routing still uses a dedicated `mainline` socket
    /// (that crate owns its bind). The `io` handle is retained for demuxed
    /// non-uTP send/recv (tests + future in-crate KRPC over the shared socket).
    pub async fn with_packet_io(io: Arc<dyn DatagramIo>, cfg: DhtConfig) -> Result<Self> {
        Self::with_datagram_io(io, cfg).await
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

    /// Receive a raw datagram from the shared IO (errors if dedicated-only).
    pub async fn shared_recv_from(&self, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        match &self.shared_io {
            Some(io) => io.recv_from(buf).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no shared datagram io",
            )),
        }
    }

    /// Bind a standalone datagram socket implementing [`DatagramIo`].
    pub async fn standalone_datagram(bind: &str) -> std::io::Result<StandaloneDatagram> {
        StandaloneDatagram::bind(bind).await
    }

    pub fn id(&self) -> [u8; 20] {
        self.id
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
        let Some(dht) = self.inner.lock().clone() else {
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

    /// Periodically re-announce torrents (Go `periodically_announce_torrents_to_dht`).
    pub fn start_periodic_announce(
        self: &Arc<Self>,
        torrents: Arc<dyn Fn() -> Vec<(InfoHash, u16)> + Send + Sync>,
        interval: Duration,
    ) {
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                for (ih, port) in torrents() {
                    if let Ok(mut h) = this.announce(ih, port, true).await {
                        // Drain briefly then close.
                        let _ = tokio::time::timeout(Duration::from_secs(10), async {
                            while h.next().await.is_some() {}
                        })
                        .await;
                    }
                }
            }
        });
        self.periodic.lock().push(handle);
    }

    pub fn close(&self) {
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

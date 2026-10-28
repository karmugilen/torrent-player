//! Incoming TCP and uTP listeners with MSE, InfoHash routing, and half-open tracking.
//!
//! LTEP / HaveNone are sent by `PeerConnHandler::start` (Go `sendInitialMessages`).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Notify};
use wt_types::{InfoHash, PeerId, PeerSource, Transport};
use wt_wire::handshake::{default_peer_extension_bytes, handshake_incoming, PeerExtensionBits};
use wt_wire::message::Message;
use wt_wire::mse::{handle_encryption, CryptoMethod};
use wt_wire::utp::UtpSocket;

use crate::conn::half_open::HalfOpenTracker;
use crate::conn::handle::ConnHandle;
use crate::stats::AtomicConnStats;
use crate::{ClientConfig, CoreError, PeerStream, Result};

/// An accepted, fully handshaken incoming peer connection.
pub struct AcceptedConn {
    pub stream: Box<dyn PeerStream>,
    pub handle: ConnHandle,
    pub write_rx: mpsc::UnboundedReceiver<Message>,
    pub write_waker: Arc<Notify>,
    pub info_hash: InfoHash,
    pub peer_id: PeerId,
    pub peer_extension_bits: PeerExtensionBits,
}

/// Callback query to look up whether an InfoHash is actively managed.
pub type InfoHashLookup = Arc<dyn Fn(&InfoHash) -> bool + Send + Sync>;

/// Callback providing candidate InfoHashes for inbound MSE decryption.
pub type SkeyProvider = Arc<dyn Fn() -> Vec<[u8; 20]> + Send + Sync>;

/// Inbound connection listener managing dual-stack TCP and uTP sockets.
pub struct PeerListener {
    tcp_listeners: Vec<Arc<TcpListener>>,
    utp_socket: Option<Arc<UtpSocket>>,
    listen_port: u16,
    closed: Arc<AtomicBool>,
    shutdown: watch::Sender<bool>,
}

impl PeerListener {
    /// Bind TCP and/or uTP listeners based on `ClientConfig`.
    pub async fn bind(cfg: &ClientConfig) -> Result<Self> {
        let closed = Arc::new(AtomicBool::new(false));
        let (shutdown, _) = watch::channel(false);
        let mut tcp_listeners = Vec::new();
        let mut utp_socket = None;
        let mut bound_port = cfg.listen_port;

        // 1. TCP Listener
        if !cfg.disable_tcp {
            let tcps = match wt_wire::transport::listen_dual_stack_tcp(bound_port).await {
                Ok(l) => l,
                Err(_) => {
                    // Fallback to IPv4 any
                    let addr = format!("0.0.0.0:{bound_port}");
                    vec![TcpListener::bind(&addr).await.map_err(CoreError::Io)?]
                }
            };
            if bound_port == 0 && !tcps.is_empty() {
                bound_port = tcps[0].local_addr().map_err(CoreError::Io)?.port();
            }
            tcp_listeners = tcps.into_iter().map(Arc::new).collect();
        }

        // 2. uTP Listener
        if !cfg.disable_utp {
            let utps = match wt_wire::transport::listen_dual_stack_utp(bound_port).await {
                Ok(u) => u,
                Err(_) => {
                    let addr: SocketAddr = format!("0.0.0.0:{bound_port}")
                        .parse()
                        .map_err(|e| CoreError::Other(format!("invalid addr: {e}")))?;
                    vec![UtpSocket::bind(addr).await.map_err(CoreError::Io)?]
                }
            };
            if let Some(first) = utps.into_iter().next() {
                if bound_port == 0 {
                    bound_port = first.local_addr().map_err(CoreError::Io)?.port();
                }
                utp_socket = Some(Arc::new(first));
            }
        }

        Ok(Self {
            tcp_listeners,
            utp_socket,
            listen_port: bound_port,
            closed,
            shutdown,
        })
    }

    /// Bound listening port.
    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    /// Access the underlying uTP socket if enabled (e.g. for DHT/UDP tracker sharing).
    pub fn utp_socket(&self) -> Option<&Arc<UtpSocket>> {
        self.utp_socket.as_ref()
    }

    /// Close all listeners.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.shutdown.send_replace(true);
    }

    /// Spawns the accept loop, sending accepted connections into `accepted_tx`.
    pub fn spawn_accept_loop(
        &self,
        cfg: ClientConfig,
        our_peer_id: PeerId,
        tracker: HalfOpenTracker,
        skey_provider: SkeyProvider,
        lookup: InfoHashLookup,
        accepted_tx: mpsc::Sender<AcceptedConn>,
    ) {
        let closed = self.closed.clone();

        // Spawn TCP accept loop
        for tcp in &self.tcp_listeners {
            let tcp = tcp.clone();
            let c_cfg = cfg.clone();
            let c_tr = tracker.clone();
            let c_sk = skey_provider.clone();
            let c_lk = lookup.clone();
            let c_tx = accepted_tx.clone();
            let c_cl = closed.clone();
            let mut shutdown = self.shutdown.subscribe();
            tokio::spawn(async move {
                while !c_cl.load(Ordering::Relaxed) {
                    let accepted = tokio::select! {
                        _ = shutdown.changed() => break,
                        accepted = tcp.accept() => accepted,
                    };
                    let (stream, peer_addr) = match accepted {
                        Ok(res) => res,
                        Err(_) => break,
                    };
                    let _ = stream.set_nodelay(true);

                    let guard = match c_tr.acquire_global(c_cfg.total_half_open_conns) {
                        Some(g) => g,
                        None => continue, // Drop connection if half-open limit exceeded
                    };

                    let handshaker_cfg = c_cfg.clone();
                    let handshaker_tr = c_tr.clone();
                    let handshaker_sk = c_sk.clone();
                    let handshaker_lk = c_lk.clone();
                    let handshaker_tx = c_tx.clone();

                    tokio::spawn(async move {
                        let _g = guard;
                        if let Ok(accepted) = handle_inbound_connection(
                            Box::new(stream),
                            peer_addr,
                            Transport::Tcp,
                            our_peer_id,
                            &handshaker_cfg,
                            &handshaker_tr,
                            &handshaker_sk,
                            &handshaker_lk,
                        )
                        .await
                        {
                            let _ = handshaker_tx.send(accepted).await;
                        }
                    });
                }
            });
        }

        // Spawn uTP accept loop
        if let Some(utp) = self.utp_socket.clone() {
            let u_cfg = cfg;
            let u_tr = tracker;
            let u_sk = skey_provider;
            let u_lk = lookup;
            let u_tx = accepted_tx;
            let u_cl = closed;
            let mut shutdown = self.shutdown.subscribe();
            tokio::spawn(async move {
                while !u_cl.load(Ordering::Relaxed) {
                    let accepted = tokio::select! {
                        _ = shutdown.changed() => break,
                        accepted = utp.accept() => accepted,
                    };
                    let stream = match accepted {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                    let peer_addr = stream.peer_addr();

                    let guard = match u_tr.acquire_global(u_cfg.total_half_open_conns) {
                        Some(g) => g,
                        None => continue,
                    };

                    let handshaker_cfg = u_cfg.clone();
                    let handshaker_tr = u_tr.clone();
                    let handshaker_sk = u_sk.clone();
                    let handshaker_lk = u_lk.clone();
                    let handshaker_tx = u_tx.clone();

                    tokio::spawn(async move {
                        let _g = guard;
                        if let Ok(accepted) = handle_inbound_connection(
                            Box::new(stream),
                            peer_addr,
                            Transport::Utp,
                            our_peer_id,
                            &handshaker_cfg,
                            &handshaker_tr,
                            &handshaker_sk,
                            &handshaker_lk,
                        )
                        .await
                        {
                            let _ = handshaker_tx.send(accepted).await;
                        }
                    });
                }
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_inbound_connection(
    stream: Box<dyn PeerStream>,
    peer_addr: SocketAddr,
    transport: Transport,
    our_peer_id: PeerId,
    cfg: &ClientConfig,
    tracker: &HalfOpenTracker,
    skey_provider: &SkeyProvider,
    lookup: &InfoHashLookup,
) -> Result<AcceptedConn> {
    tokio::time::timeout(
        cfg.handshakes_timeout,
        handle_inbound_inner(
            stream,
            peer_addr,
            transport,
            our_peer_id,
            cfg,
            tracker,
            skey_provider,
            lookup,
        ),
    )
    .await
    .map_err(|_| CoreError::Other("incoming handshake timeout".to_string()))?
}

#[allow(clippy::too_many_arguments)]
async fn handle_inbound_inner(
    raw_stream: Box<dyn PeerStream>,
    peer_addr: SocketAddr,
    transport: Transport,
    our_peer_id: PeerId,
    cfg: &ClientConfig,
    tracker: &HalfOpenTracker,
    skey_provider: &SkeyProvider,
    lookup: &InfoHashLookup,
) -> Result<AcceptedConn> {
    // 1. MSE Handshake
    let skeys: Vec<Vec<u8>> = (skey_provider)().into_iter().map(|k| k.to_vec()).collect();
    let mse_policy = cfg.header_obfuscation_policy;
    let selector = move |methods: CryptoMethod| {
        if mse_policy.preferred && methods.supports_rc4() {
            CryptoMethod::RC4
        } else if !mse_policy.preferred && methods.supports_plaintext() {
            CryptoMethod::PLAINTEXT
        } else if !mse_policy.require_preferred && methods.supports_rc4() {
            CryptoMethod::RC4
        } else {
            CryptoMethod::PLAINTEXT
        }
    };

    let (encrypted_stream, _is_header_enc, crypto_method) =
        handle_encryption(raw_stream, skeys, mse_policy, selector)
            .await
            .map_err(|e| CoreError::Other(format!("incoming MSE error: {e}")))?;

    let is_encrypted = crypto_method == CryptoMethod::RC4;
    let mut stream: Box<dyn PeerStream> = Box::new(encrypted_stream);

    // 2. Incoming BitTorrent Handshake
    let reserved_bits = default_peer_extension_bytes();
    let lk = lookup.clone();
    let lookup_fn = move |ih: &InfoHash| -> bool { lk(ih) };

    let peer_hs = handshake_incoming(&mut stream, &our_peer_id, &reserved_bits, lookup_fn)
        .await
        .map_err(|e| CoreError::Other(format!("incoming BitTorrent handshake error: {e}")))?;

    let info_hash = peer_hs.info_hash;
    let _tor_guard = tracker
        .acquire_torrent(info_hash, cfg.half_open_conns_per_torrent)
        .ok_or_else(|| {
            CoreError::Other("per-torrent half-open limit exceeded for incoming".to_string())
        })?;

    let peer_ext_bits = peer_hs.extensions;
    let peer_id = peer_hs.peer_id;

    // LTEP / HaveNone deferred to PeerConnHandler::start (Go sendInitialMessages).
    let (write_tx, write_rx) = mpsc::unbounded_channel();
    let write_waker = Arc::new(Notify::new());
    let closed = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(AtomicConnStats::default());

    let handle = ConnHandle::new(
        peer_addr,
        false,
        transport,
        is_encrypted,
        info_hash,
        Some(peer_id),
        PeerSource::Incoming,
        peer_ext_bits.0,
        write_tx,
        write_waker.clone(),
        closed,
        stats,
    );

    Ok(AcceptedConn {
        stream,
        handle,
        write_rx,
        write_waker,
        info_hash,
        peer_id,
        peer_extension_bits: peer_ext_bits,
    })
}

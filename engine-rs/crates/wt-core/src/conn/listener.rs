//! Incoming TCP and uTP listeners with MSE, InfoHash routing, and half-open tracking.
//!
//! LTEP / HaveNone are sent by `PeerConnHandler::start` (Go `sendInitialMessages`).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Notify};
use wt_types::{InfoHash, PeerId, PeerSource, Transport};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt as _;
use wt_wire::handshake::{
    default_peer_extension_bytes, HandshakeResult, PeerExtensionBits, EXTENSION_BIT_V2_UPGRADE,
    HANDSHAKE_LEN, PROTOCOL_HEADER,
    write_handshake,
};
use wt_wire::message::Message;
use wt_wire::mse::{
    default_crypto_selector, handle_encryption, CryptoMethod, HeaderObfuscationPolicy,
};
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

/// Callback answering whether the torrent an inbound handshake matched has a
/// v2 hash (hybrid or pure v2). Our reply then sets the BEP 52 v2-upgrade
/// bit, so a hybrid peer that reached us by the v1 hash sees us as v2 capable.
pub type V2Lookup = Arc<dyn Fn(&InfoHash) -> bool + Send + Sync>;

type SharedV2Lookup = Arc<parking_lot::RwLock<Option<V2Lookup>>>;

/// Callback providing candidate InfoHashes for inbound MSE decryption.
pub type SkeyProvider = Arc<dyn Fn() -> Vec<[u8; 20]> + Send + Sync>;

/// Inbound connection listener managing dual-stack TCP and uTP sockets.
pub struct PeerListener {
    tcp_listeners: Vec<Arc<TcpListener>>,
    /// IPv4 uTP socket: dials to IPv4 peers, DHT and UDP trackers share it.
    utp_socket: Option<Arc<UtpSocket>>,
    /// IPv6 uTP socket on the same port, when the host has IPv6.
    utp_socket_v6: Option<Arc<UtpSocket>>,
    listen_port: u16,
    closed: Arc<AtomicBool>,
    shutdown: watch::Sender<bool>,
    v2_lookup: SharedV2Lookup,
}

impl PeerListener {
    /// Bind TCP and/or uTP listeners based on `ClientConfig`.
    pub async fn bind(cfg: &ClientConfig) -> Result<Self> {
        let closed = Arc::new(AtomicBool::new(false));
        let (shutdown, _) = watch::channel(false);
        let mut tcp_listeners = Vec::new();
        let mut utp_socket = None;
        let mut utp_socket_v6 = None;
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
            for sock in utps {
                let local = sock.local_addr().map_err(CoreError::Io)?;
                if bound_port == 0 {
                    bound_port = local.port();
                }
                if local.is_ipv4() {
                    utp_socket.get_or_insert_with(|| Arc::new(sock));
                } else {
                    utp_socket_v6.get_or_insert_with(|| Arc::new(sock));
                }
            }
        }

        Ok(Self {
            tcp_listeners,
            utp_socket,
            utp_socket_v6,
            listen_port: bound_port,
            closed,
            shutdown,
            v2_lookup: Arc::default(),
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

    /// The uTP socket for dialing `addr`: IPv6 peers need the IPv6 socket.
    pub fn utp_socket_for(&self, addr: SocketAddr) -> Option<&Arc<UtpSocket>> {
        if addr.is_ipv6() {
            self.utp_socket_v6.as_ref()
        } else {
            self.utp_socket.as_ref()
        }
    }

    /// Close all listeners.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.shutdown.send_replace(true);
    }

    /// Install the v2 lookup used by inbound handshake replies. It applies to
    /// connections accepted from now on, whether or not the accept loop runs.
    pub fn set_v2_lookup(&self, lookup: V2Lookup) {
        *self.v2_lookup.write() = Some(lookup);
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
        let v2_lookup = self.v2_lookup.clone();

        // Spawn TCP accept loop
        for tcp in &self.tcp_listeners {
            let tcp = tcp.clone();
            let c_cfg = cfg.clone();
            let c_tr = tracker.clone();
            let c_sk = skey_provider.clone();
            let c_lk = lookup.clone();
            let c_v2 = v2_lookup.clone();
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
                    let handshaker_v2 = c_v2.clone();
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
                            &handshaker_v2,
                        )
                        .await
                        {
                            let _ = handshaker_tx.send(accepted).await;
                        }
                    });
                }
            });
        }

        // Spawn uTP accept loops (IPv4 and IPv6)
        for utp in self.utp_socket.iter().chain(self.utp_socket_v6.iter()).cloned() {
            let u_cfg = cfg.clone();
            let u_tr = tracker.clone();
            let u_sk = skey_provider.clone();
            let u_lk = lookup.clone();
            let u_v2 = v2_lookup.clone();
            let u_tx = accepted_tx.clone();
            let u_cl = closed.clone();
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
                    let handshaker_v2 = u_v2.clone();
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
                            &handshaker_v2,
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
    v2_lookup: &SharedV2Lookup,
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
            v2_lookup,
        ),
    )
    .await
    .map_err(|_| CoreError::Other("incoming handshake timeout".to_string()))?
}

/// Our reply bits for an inbound handshake on `info_hash`: the defaults, plus
/// the BEP 52 v2-upgrade bit when the matched torrent has a v2 hash.
fn inbound_reply_bits(info_hash: &InfoHash, v2_lookup: Option<&V2Lookup>) -> PeerExtensionBits {
    let mut bits = default_peer_extension_bytes();
    if v2_lookup.is_some_and(|has_v2| has_v2(info_hash)) {
        bits.set_bit(EXTENSION_BIT_V2_UPGRADE, true);
    }
    bits
}

/// `wt_wire::handshake_incoming`, except our reserved bits are chosen after
/// the info hash is known (the v2-upgrade bit depends on the torrent).
async fn inbound_bt_handshake<S>(
    stream: &mut S,
    our_peer_id: &PeerId,
    lookup: &InfoHashLookup,
    v2_lookup: Option<&V2Lookup>,
) -> std::io::Result<HandshakeResult>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use std::io::{Error, ErrorKind};
    let mut buf = [0u8; HANDSHAKE_LEN];
    stream.read_exact(&mut buf).await?;
    if buf[..20] != PROTOCOL_HEADER[..] {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unexpected protocol header: {:?}", &buf[..20]),
        ));
    }
    let mut ext = [0u8; 8];
    ext.copy_from_slice(&buf[20..28]);
    let mut ih = [0u8; 20];
    ih.copy_from_slice(&buf[28..48]);
    let info_hash = InfoHash(ih);
    if !lookup(&info_hash) {
        let _ = stream.shutdown().await;
        return Err(Error::new(ErrorKind::NotFound, format!("infohash not found: {info_hash}")));
    }
    let mut peer_id = [0u8; 20];
    peer_id.copy_from_slice(&buf[48..68]);

    let bits = inbound_reply_bits(&info_hash, v2_lookup);
    write_handshake(stream, &info_hash, our_peer_id, &bits).await?;
    stream.flush().await?;
    Ok(HandshakeResult { extensions: PeerExtensionBits(ext), info_hash, peer_id })
}

/// Payload cipher for an inbound MSE handshake. Go `mse.DefaultCryptoSelector`:
/// plaintext whenever the dialer offers it, for speed; the header stays
/// obfuscated either way. Choosing RC4 whenever obfuscation was preferred,
/// which is the default, encrypted every byte of every upload in pure Rust
/// and capped seeding near 220 MiB/s on loopback. A policy that requires
/// obfuscation keeps RC4, as the dialer then offers only RC4.
fn select_inbound_crypto(policy: HeaderObfuscationPolicy, offered: CryptoMethod) -> CryptoMethod {
    if policy.preferred && policy.require_preferred && offered.supports_rc4() {
        CryptoMethod::RC4
    } else {
        default_crypto_selector(offered)
    }
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
    v2_lookup: &SharedV2Lookup,
) -> Result<AcceptedConn> {
    // 1. MSE Handshake
    let skeys: Vec<Vec<u8>> = (skey_provider)().into_iter().map(|k| k.to_vec()).collect();
    let mse_policy = cfg.header_obfuscation_policy;
    let selector = move |methods: CryptoMethod| select_inbound_crypto(mse_policy, methods);

    let (encrypted_stream, _is_header_enc, crypto_method) =
        handle_encryption(raw_stream, skeys, mse_policy, selector)
            .await
            .map_err(|e| CoreError::Other(format!("incoming MSE error: {e}")))?;

    let is_encrypted = crypto_method == CryptoMethod::RC4;
    let mut stream: Box<dyn PeerStream> = Box::new(encrypted_stream);

    // 2. Incoming BitTorrent Handshake
    let v2 = v2_lookup.read().clone();
    let peer_hs = inbound_bt_handshake(&mut stream, &our_peer_id, lookup, v2.as_ref())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(preferred: bool, require_preferred: bool) -> HeaderObfuscationPolicy {
        HeaderObfuscationPolicy {
            preferred,
            require_preferred,
        }
    }

    #[test]
    fn inbound_prefers_plaintext_payload_like_go() {
        // Default policy: obfuscated header, plaintext payload when offered.
        let default = HeaderObfuscationPolicy::default();
        assert_eq!(select_inbound_crypto(default, CryptoMethod::ALL), CryptoMethod::PLAINTEXT);
        assert_eq!(select_inbound_crypto(policy(false, false), CryptoMethod::ALL), CryptoMethod::PLAINTEXT);
        // RC4 only when it is the only offer.
        assert_eq!(select_inbound_crypto(default, CryptoMethod::RC4), CryptoMethod::RC4);
    }

    #[test]
    fn inbound_required_obfuscation_keeps_rc4() {
        let required = policy(true, true);
        assert_eq!(select_inbound_crypto(required, CryptoMethod::ALL), CryptoMethod::RC4);
        assert_eq!(select_inbound_crypto(required, CryptoMethod::PLAINTEXT), CryptoMethod::PLAINTEXT);
    }
}

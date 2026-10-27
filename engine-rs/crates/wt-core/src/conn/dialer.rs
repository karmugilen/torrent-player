//! Outgoing peer connection dialing, encryption, and handshaking.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use bytes::BytesMut;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Notify};
use wt_types::{InfoHash, PeerId, PeerSource, Transport};
use wt_wire::ext::{ExtendedHandshake, EXTENSION_NAME_METADATA, EXTENSION_NAME_PEX};
use wt_wire::handshake::{default_peer_extension_bytes, handshake, PeerExtensionBits};
use wt_wire::message::Message;
use wt_wire::mse::{self, CryptoMethod};
use wt_wire::utp::UtpSocket;

use crate::conn::half_open::HalfOpenTracker;
use crate::conn::handle::ConnHandle;
use crate::ratelimit::Limiter;
use crate::stats::AtomicConnStats;
use crate::{ClientConfig, CoreError, PeerStream, Result, TcpConnHook};

/// Outcome of a successful outgoing dial and handshake.
pub struct DialOutcome {
    pub stream: Box<dyn PeerStream>,
    pub handle: ConnHandle,
    pub write_rx: mpsc::Receiver<Message>,
    pub write_waker: Arc<Notify>,
    pub peer_extension_bits: PeerExtensionBits,
    pub peer_id: PeerId,
}

/// Parameters for initiating an outgoing peer connection.
pub struct DialParams<'a> {
    pub remote_addr: SocketAddr,
    pub info_hash: InfoHash,
    pub our_peer_id: PeerId,
    pub transport: Option<Transport>,
    pub source: PeerSource,
    pub metadata_size: Option<u32>,
    pub cfg: &'a ClientConfig,
    pub tracker: &'a HalfOpenTracker,
    pub dial_limiter: Option<&'a Limiter>,
    pub tcp_conn_hook: Option<&'a TcpConnHook>,
    pub utp_socket: Option<&'a Arc<UtpSocket>>,
    pub require_plain: bool,
}

/// Dials an outgoing peer, negotiates MSE, and completes BitTorrent + LTEP handshakes.
pub async fn dial_peer(params: DialParams<'_>) -> Result<DialOutcome> {
    let cfg = params.cfg;

    // 1. Half-open concurrency limits
    let _global_guard = params
        .tracker
        .acquire_global(cfg.total_half_open_conns)
        .ok_or_else(|| CoreError::Other("global half-open limit exceeded".to_string()))?;

    let _torrent_guard = params
        .tracker
        .acquire_torrent(params.info_hash, cfg.half_open_conns_per_torrent)
        .ok_or_else(|| CoreError::Other("torrent half-open limit exceeded".to_string()))?;

    // 2. Dial rate limiter
    if let Some(limiter) = params.dial_limiter {
        if let Some(delay) = limiter.reserve_delay(1) {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
    }

    // 3. Dial transport
    let dial_timeout = cfg.nominal_dial_timeout.max(cfg.min_dial_timeout);
    let chosen_transport = params.transport.unwrap_or(if !cfg.disable_tcp {
        Transport::Tcp
    } else {
        Transport::Utp
    });

    let raw_stream: Box<dyn PeerStream> = match chosen_transport {
        Transport::Tcp => {
            if cfg.disable_tcp {
                return Err(CoreError::Other("TCP transport disabled".to_string()));
            }
            let tcp = tokio::time::timeout(
                dial_timeout,
                tokio::net::TcpStream::connect(params.remote_addr),
            )
            .await
            .map_err(|_| CoreError::Other("TCP connect timeout".to_string()))?
            .map_err(CoreError::Io)?;

            if let Some(hook) = params.tcp_conn_hook {
                hook(tcp)
            } else {
                Box::new(tcp)
            }
        }
        Transport::Utp => {
            if cfg.disable_utp {
                return Err(CoreError::Other("uTP transport disabled".to_string()));
            }
            let (sock, _temp_sock) = if let Some(s) = params.utp_socket {
                ((*s).clone(), None)
            } else {
                let bind_addr: SocketAddr = "0.0.0.0:0"
                    .parse()
                    .map_err(|e| CoreError::Other(format!("invalid addr: {e}")))?;
                let s = Arc::new(UtpSocket::bind(bind_addr).await.map_err(CoreError::Io)?);
                (s.clone(), Some(s))
            };

            let utp = wt_wire::transport::dial_utp(&sock, params.remote_addr, dial_timeout)
                .await
                .map_err(CoreError::Io)?;

            Box::new(utp)
        }
        other => {
            return Err(CoreError::Other(format!(
                "unsupported transport for dial_peer: {other:?}"
            )));
        }
    };

    // 4. Message Stream Encryption (MSE) negotiation
    let (mut stream, is_encrypted): (Box<dyn PeerStream>, bool) = if params.require_plain {
        (raw_stream, false)
    } else {
        let mse_fut =
            mse::initiate_handshake(raw_stream, &params.info_hash.0, None, CryptoMethod::ALL);
        let (encrypted_stream, crypto_method) =
            tokio::time::timeout(cfg.handshakes_timeout, mse_fut)
                .await
                .map_err(|_| CoreError::Other("MSE handshake timeout".to_string()))?
                .map_err(|e| CoreError::Other(format!("MSE negotiation failed: {e}")))?;

        let is_enc = crypto_method == CryptoMethod::RC4;
        (Box::new(encrypted_stream), is_enc)
    };

    // 5. BitTorrent protocol handshake
    let reserved_bits = default_peer_extension_bytes();
    let hs_fut = handshake(
        &mut stream,
        &params.info_hash,
        &params.our_peer_id,
        &reserved_bits,
    );
    let peer_hs = tokio::time::timeout(cfg.handshakes_timeout, hs_fut)
        .await
        .map_err(|_| CoreError::Other("BitTorrent handshake timeout".to_string()))?
        .map_err(|e| CoreError::Other(format!("BitTorrent handshake failed: {e}")))?;

    if peer_hs.info_hash != params.info_hash {
        return Err(CoreError::Other("peer returned mismatched InfoHash".into()));
    }

    let peer_ext_bits = peer_hs.extensions;
    let peer_id = peer_hs.peer_id;

    // 6. Send initial LTEP extension handshake if peer supports LTEP
    if peer_ext_bits.supports_extended() {
        let mut extensions_map = BTreeMap::new();
        extensions_map.insert(EXTENSION_NAME_METADATA.to_string(), 1);
        if !cfg.disable_pex {
            extensions_map.insert(EXTENSION_NAME_PEX.to_string(), 2);
        }

        let ext_dict = ExtendedHandshake {
            m: extensions_map,
            v: Some(cfg.extended_handshake_client_version.clone()),
            p: Some(cfg.listen_port),
            yourip: Some(params.remote_addr.ip()),
            ipv4: None,
            ipv6: None,
            reqq: Some(250),
            metadata_size: params.metadata_size,
            upload_only: Some(cfg.seed),
            encryption: Some(is_encrypted),
        };

        let ext_msg = Message::Extended {
            id: 0,
            payload: ext_dict.encode(),
        };

        let mut out_bytes = BytesMut::new();
        ext_msg.encode(&mut out_bytes);
        let _ = stream.write_all(&out_bytes).await;
        let _ = stream.flush().await;
    }

    // 7. Construct ConnHandle
    let (write_tx, write_rx) = mpsc::channel(256);
    let write_waker = Arc::new(Notify::new());
    let closed = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(AtomicConnStats::default());

    let handle = ConnHandle::new(
        params.remote_addr,
        true,
        chosen_transport,
        is_encrypted,
        params.info_hash,
        Some(peer_id),
        params.source,
        write_tx,
        write_waker.clone(),
        closed,
        stats,
    );

    Ok(DialOutcome {
        stream,
        handle,
        write_rx,
        write_waker,
        peer_extension_bits: peer_ext_bits,
        peer_id,
    })
}

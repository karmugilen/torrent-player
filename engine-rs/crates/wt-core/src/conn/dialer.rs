//! Outgoing peer connection dialing, encryption, and handshaking.
//!
//! Matches anacrolix `dialAndCompleteHandshake` + `dialPool`:
//! - Race enabled transports (TCP+uTP) under one half-open slot; take the first
//!   successful dial (Go `dialPool.getFirst`).
//! - Try preferred header-obfuscation, then fall back on the *same* winning
//!   transport when `RequirePreferred` is false.
//! LTEP / HaveNone / Unchoke are sent by `PeerConnHandler::start`.

use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Notify};
use wt_types::{InfoHash, PeerId, PeerSource, Transport};
use wt_wire::handshake::{default_peer_extension_bytes, handshake, PeerExtensionBits};
use wt_wire::message::Message;
use wt_wire::mse::{self, CryptoMethod};
use wt_wire::utp::UtpSocket;

use crate::conn::half_open::HalfOpenTracker;
use crate::conn::handle::ConnHandle;
use crate::ratelimit::Limiter;
use crate::stats::AtomicConnStats;
use crate::{ClientConfig, CoreError, PeerStream, Result, TcpConnHook};

/// Legacy bounded-queue size (kept for API/compat). Writer is unbounded so Piece
/// replies are never try_send-dropped under PeerMaxRequests / LTEP `reqq` pipelines.
pub const OUTBOUND_MSG_QUEUE: usize = 8192;

/// Outcome of a successful outgoing dial and handshake.
pub struct DialOutcome {
    pub stream: Box<dyn PeerStream>,
    pub handle: ConnHandle,
    pub write_rx: mpsc::UnboundedReceiver<Message>,
    pub write_waker: Arc<Notify>,
    pub peer_extension_bits: PeerExtensionBits,
    pub peer_id: PeerId,
}

/// Parameters for initiating an outgoing peer connection.
pub struct DialParams<'a> {
    pub remote_addr: SocketAddr,
    pub info_hash: InfoHash,
    pub our_peer_id: PeerId,
    /// `None` = race every enabled transport (Go dialPool). `Some` = only that one.
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

/// Dials an outgoing peer, negotiates MSE (with plaintext/encrypted fallback), and
/// completes the BitTorrent handshake. LTEP is deferred to `PeerConnHandler::start`.
pub async fn dial_peer(params: DialParams<'_>) -> Result<DialOutcome> {
    let cfg = params.cfg;

    let _global_guard = params
        .tracker
        .acquire_global(cfg.total_half_open_conns)
        .ok_or_else(|| CoreError::Other("global half-open limit exceeded".to_string()))?;

    let _torrent_guard = params
        .tracker
        .acquire_torrent(params.info_hash, cfg.half_open_conns_per_torrent)
        .ok_or_else(|| CoreError::Other("torrent half-open limit exceeded".to_string()))?;

    if let Some(limiter) = params.dial_limiter {
        if let Some(delay) = limiter.reserve_delay(1) {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
    }

    let prefer_obfuscated = if params.require_plain {
        false
    } else {
        cfg.header_obfuscation_policy.preferred
    };
    let allow_fallback =
        !params.require_plain && !cfg.header_obfuscation_policy.require_preferred;

    let dial_timeout = cfg.nominal_dial_timeout.max(cfg.min_dial_timeout);

    // Go dialPool: race transports, keep the first successful dialer for fallback.
    let (raw_stream, winning_transport) = dial_first_raw(&params, dial_timeout).await?;

    match complete_bt_handshake(&params, raw_stream, winning_transport, prefer_obfuscated).await {
        Ok(outcome) => Ok(outcome),
        Err(first_err) if allow_fallback => {
            tracing::debug!(
                addr = %params.remote_addr,
                transport = ?winning_transport,
                obfuscated = prefer_obfuscated,
                error = %first_err,
                "preferred header obfuscation failed; redialing same transport with fallback"
            );
            let (raw2, transport2) =
                dial_raw_one(&params, winning_transport, dial_timeout).await.map_err(|e| {
                    CoreError::Other(format!(
                        "handshake failed (preferred obfuscation={prefer_obfuscated}: {first_err}; fallback redial: {e})"
                    ))
                })?;
            complete_bt_handshake(&params, raw2, transport2, !prefer_obfuscated)
                .await
                .map_err(|fallback_err| {
                    CoreError::Other(format!(
                        "handshake failed (preferred obfuscation={prefer_obfuscated}: {first_err}; fallback: {fallback_err})"
                    ))
                })
        }
        Err(e) => Err(e),
    }
}

/// Go `dialPool`: start all candidate dialers, return the first success.
async fn dial_first_raw(
    params: &DialParams<'_>,
    dial_timeout: Duration,
) -> Result<(Box<dyn PeerStream>, Transport)> {
    let candidates = candidate_transports(params);
    if candidates.is_empty() {
        return Err(CoreError::Other("no dial transports enabled".into()));
    }
    if candidates.len() == 1 {
        let t = candidates[0];
        let stream = dial_raw(params, t, dial_timeout).await?;
        return Ok((stream, t));
    }

    let n = candidates.len();
    let (tx, mut rx) = mpsc::channel::<(Transport, Result<Box<dyn PeerStream>>)>(n);
    for transport in candidates {
        let tx = tx.clone();
        let addr = params.remote_addr;
        let disable_tcp = params.cfg.disable_tcp;
        let disable_utp = params.cfg.disable_utp;
        let hook = params.tcp_conn_hook.cloned();
        let utp = params.utp_socket.cloned();
        tokio::spawn(async move {
            let result = dial_raw_owned(
                addr,
                transport,
                dial_timeout,
                disable_tcp,
                disable_utp,
                hook.as_ref(),
                utp.as_ref(),
            )
            .await;
            let _ = tx.send((transport, result)).await;
        });
    }
    drop(tx);

    let mut last_err = None;
    let mut pending = n;
    while pending > 0 {
        let Some((transport, result)) = rx.recv().await else {
            break;
        };
        pending -= 1;
        match result {
            Ok(stream) => {
                // Go dialPool.startDrainer: drop late successes so sockets close.
                if pending > 0 {
                    tokio::spawn(async move {
                        while let Some((_t, other)) = rx.recv().await {
                            drop(other);
                        }
                    });
                }
                return Ok((stream, transport));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| CoreError::Other("all initial dials failed".into())))
}

fn candidate_transports(params: &DialParams<'_>) -> Vec<Transport> {
    if let Some(t) = params.transport {
        return vec![t];
    }
    let mut out = Vec::with_capacity(2);
    if !params.cfg.disable_tcp {
        out.push(Transport::Tcp);
    }
    if !params.cfg.disable_utp {
        out.push(Transport::Utp);
    }
    out
}

async fn dial_raw_one(
    params: &DialParams<'_>,
    transport: Transport,
    dial_timeout: Duration,
) -> Result<(Box<dyn PeerStream>, Transport)> {
    let stream = dial_raw(params, transport, dial_timeout).await?;
    Ok((stream, transport))
}

async fn complete_bt_handshake(
    params: &DialParams<'_>,
    raw_stream: Box<dyn PeerStream>,
    chosen_transport: Transport,
    use_header_obfuscation: bool,
) -> Result<DialOutcome> {
    let cfg = params.cfg;

    let (mut stream, is_encrypted): (Box<dyn PeerStream>, bool) = if use_header_obfuscation {
        let provides = if cfg.header_obfuscation_policy.preferred
            && cfg.header_obfuscation_policy.require_preferred
        {
            CryptoMethod::RC4
        } else {
            CryptoMethod::ALL
        };
        let mse_fut = mse::initiate_handshake(raw_stream, &params.info_hash.0, None, provides);
        let (encrypted_stream, crypto_method) =
            tokio::time::timeout(cfg.handshakes_timeout, mse_fut)
                .await
                .map_err(|_| CoreError::Other("MSE handshake timeout".to_string()))?
                .map_err(|e| CoreError::Other(format!("MSE negotiation failed: {e}")))?;
        (
            Box::new(encrypted_stream),
            crypto_method == CryptoMethod::RC4,
        )
    } else {
        (raw_stream, false)
    };

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

    let (write_tx, write_rx) = mpsc::unbounded_channel();
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
        peer_ext_bits.0,
        write_tx,
        write_waker.clone(),
        closed,
        stats,
    );

    let _ = params.metadata_size;

    Ok(DialOutcome {
        stream,
        handle,
        write_rx,
        write_waker,
        peer_extension_bits: peer_ext_bits,
        peer_id,
    })
}

async fn dial_raw(
    params: &DialParams<'_>,
    chosen_transport: Transport,
    dial_timeout: Duration,
) -> Result<Box<dyn PeerStream>> {
    dial_raw_owned(
        params.remote_addr,
        chosen_transport,
        dial_timeout,
        params.cfg.disable_tcp,
        params.cfg.disable_utp,
        params.tcp_conn_hook,
        params.utp_socket,
    )
    .await
}

async fn dial_raw_owned(
    remote_addr: SocketAddr,
    chosen_transport: Transport,
    dial_timeout: Duration,
    disable_tcp: bool,
    disable_utp: bool,
    tcp_conn_hook: Option<&TcpConnHook>,
    utp_socket: Option<&Arc<UtpSocket>>,
) -> Result<Box<dyn PeerStream>> {
    match chosen_transport {
        Transport::Tcp => {
            if disable_tcp {
                return Err(CoreError::Other("TCP transport disabled".to_string()));
            }
            let tcp = tokio::time::timeout(dial_timeout, tokio::net::TcpStream::connect(remote_addr))
                .await
                .map_err(|_| CoreError::Other("TCP connect timeout".to_string()))?
                .map_err(CoreError::Io)?;

            if let Some(hook) = tcp_conn_hook {
                Ok(hook(tcp))
            } else {
                Ok(Box::new(tcp))
            }
        }
        Transport::Utp => {
            if disable_utp {
                return Err(CoreError::Other("uTP transport disabled".to_string()));
            }
            let (sock, _temp_sock) = if let Some(s) = utp_socket {
                ((*s).clone(), None)
            } else {
                let bind_addr: SocketAddr = "0.0.0.0:0"
                    .parse()
                    .map_err(|e| CoreError::Other(format!("invalid addr: {e}")))?;
                let s = Arc::new(UtpSocket::bind(bind_addr).await.map_err(CoreError::Io)?);
                (s.clone(), Some(s))
            };

            let utp = wt_wire::transport::dial_utp(&sock, remote_addr, dial_timeout)
                .await
                .map_err(CoreError::Io)?;
            Ok(Box::new(utp))
        }
        other => Err(CoreError::Other(format!(
            "unsupported transport for dial_peer: {other:?}"
        ))),
    }
}

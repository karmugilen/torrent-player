//! Outgoing peer connection dialing, encryption, and handshaking.
//!
//! Matches anacrolix `dialAndCompleteHandshake` + `dialPool`:
//! - Race enabled transports (TCP+uTP) under one half-open slot; take the first
//!   successful dial (Go `dialPool.getFirst`).
//! - Try preferred header-obfuscation, then fall back on the *same* winning
//!   transport when `RequirePreferred` is false.
//!
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

/// Capacity for the outbound message queue. Sized for Go `localClientReqq` (1024).
/// Legacy size constant (writer is unbounded).
pub const OUTBOUND_MSG_QUEUE: usize = 1024;

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

    // Race transports; keep a late second success for handshake fallback so we
    // do not drop a live uTP/TCP socket only to redial it (Go has 9/35 uTP on
    // Ubuntu; Rust was sticking at 0 uTP after draining the race loser).
    let (raw_stream, winning_transport, mut spare) =
        dial_first_raw(&params, dial_timeout).await?;

    // Fast-fail the *preferred* MSE attempt (2s). Ubuntu sees ~20+ MSE
    // early-EOF/timeouts per minute when Preferred=true; waiting the full
    // 5s handshakes_timeout before plaintext fallback burned half-open
    // slots and left dial_ok ~14 vs Go ~32. Fallback below still uses the
    // full handshake timeout for plaintext / other-transport retries.
    let prefer_hs_timeout = if prefer_obfuscated && allow_fallback {
        Duration::from_secs(2).min(cfg.handshakes_timeout)
    } else {
        cfg.handshakes_timeout
    };

    match complete_bt_handshake(
        &params,
        raw_stream,
        winning_transport,
        prefer_obfuscated,
        prefer_hs_timeout,
    )
    .await
    {
        Ok(outcome) => Ok(outcome),
        Err(first_err) if allow_fallback => {
            tracing::debug!(
                addr = %params.remote_addr,
                transport = ?winning_transport,
                obfuscated = prefer_obfuscated,
                error = %first_err,
                "preferred header obfuscation failed; trying spare/fallback"
            );
            let full_hs = cfg.handshakes_timeout;
            // 0) Spare race loser (already connected): try opposite obfuscation
            // on the live socket, then redial that transport with preferred.
            if let Some((spare_stream, spare_transport)) = spare.take() {
                if let Ok(outcome) = complete_bt_handshake(
                    &params,
                    spare_stream,
                    spare_transport,
                    !prefer_obfuscated,
                    full_hs,
                )
                .await
                {
                    return Ok(outcome);
                }
                if let Ok((raw, t)) = dial_raw_one(&params, spare_transport, dial_timeout).await {
                    if let Ok(outcome) = complete_bt_handshake(
                        &params,
                        raw,
                        t,
                        prefer_obfuscated,
                        full_hs,
                    )
                    .await
                    {
                        return Ok(outcome);
                    }
                }
            }
            // 1) Same transport, opposite obfuscation (Go).
            if let Ok((raw2, transport2)) =
                dial_raw_one(&params, winning_transport, dial_timeout).await
            {
                if let Ok(outcome) = complete_bt_handshake(
                    &params,
                    raw2,
                    transport2,
                    !prefer_obfuscated,
                    full_hs,
                )
                .await
                {
                    return Ok(outcome);
                }
            }
            // 2) Other transports: try both obfuscation modes (unique).
            // When preferred=false, `[false, prefer]` was `[false, false]` and
            // never attempted MSE — Ubuntu r47 then logged ~27 bt_hs fails on
            // peers that require encryption.
            let obfuscation_order = if prefer_obfuscated {
                [true, false]
            } else {
                [false, true]
            };
            for transport in candidate_transports(&params) {
                if transport == winning_transport {
                    continue;
                }
                for obfuscated in obfuscation_order {
                    let Ok((raw, t)) = dial_raw_one(&params, transport, dial_timeout).await else {
                        continue;
                    };
                    if let Ok(outcome) =
                        complete_bt_handshake(&params, raw, t, obfuscated, full_hs).await
                    {
                        return Ok(outcome);
                    }
                }
            }
            Err(CoreError::Other(format!(
                "handshake failed (preferred obfuscation={prefer_obfuscated}: {first_err}; same+other transport fallbacks exhausted)"
            )))
        }
        Err(e) => Err(e),
    }
}

/// Go `dialPool`: start all candidate dialers, return the first success.
/// Also returns a late second success (if any) so handshake fallback can use
/// the already-open socket instead of immediately draining it.
async fn dial_first_raw(
    params: &DialParams<'_>,
    dial_timeout: Duration,
) -> Result<(
    Box<dyn PeerStream>,
    Transport,
    Option<(Box<dyn PeerStream>, Transport)>,
)> {
    let candidates = candidate_transports(params);
    if candidates.is_empty() {
        return Err(CoreError::Other("no dial transports enabled".into()));
    }
    if candidates.len() == 1 {
        let t = candidates[0];
        let stream = dial_raw(params, t, dial_timeout).await?;
        return Ok((stream, t, None));
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
    let mut winner: Option<(Box<dyn PeerStream>, Transport)> = None;
    while pending > 0 {
        let Some((transport, result)) = rx.recv().await else {
            break;
        };
        pending -= 1;
        match result {
            Ok(stream) => {
                if winner.is_none() {
                    winner = Some((stream, transport));
                    // Wait for the other transport so we can keep it as a
                    // handshake spare. Prefer TCP when both succeed: public
                    // uTP piece throughput is still weak (107.175 on uTP often
                    // stays at downloaded=0 while the same peer on TCP feeds
                    // multi-MiB). 750ms covers slow TCP connects that lose the
                    // race to a fast uTP SYN-ACK.
                    if pending > 0 {
                        let wait = if winner
                            .as_ref()
                            .is_some_and(|(_, t)| *t == Transport::Utp)
                        {
                            Duration::from_millis(750)
                        } else {
                            Duration::from_millis(250)
                        };
                        match tokio::time::timeout(wait, rx.recv()).await {
                            Ok(Some((t2, Ok(s2)))) => {
                                pending = pending.saturating_sub(1);
                                drain_remaining_dials(rx, pending);
                                let (w_stream, w_t) = winner.take().unwrap();
                                if w_t == Transport::Utp && t2 == Transport::Tcp {
                                    return Ok((s2, t2, Some((w_stream, w_t))));
                                }
                                return Ok((w_stream, w_t, Some((s2, t2))));
                            }
                            Ok(Some((_t2, Err(e)))) => {
                                pending = pending.saturating_sub(1);
                                let _ = e;
                            }
                            Ok(None) | Err(_) => {}
                        }
                    }
                    drain_remaining_dials(rx, pending);
                    let (w_stream, w_t) = winner.take().unwrap();
                    return Ok((w_stream, w_t, None));
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    if let Some((stream, transport)) = winner {
        return Ok((stream, transport, None));
    }
    Err(last_err.unwrap_or_else(|| CoreError::Other("all initial dials failed".into())))
}

fn drain_remaining_dials(
    mut rx: mpsc::Receiver<(Transport, Result<Box<dyn PeerStream>>)>,
    pending: usize,
) {
    if pending == 0 {
        return;
    }
    tokio::spawn(async move {
        while let Some((_t, other)) = rx.recv().await {
            drop(other);
        }
    });
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
    hs_timeout: Duration,
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
        let (encrypted_stream, crypto_method) = tokio::time::timeout(hs_timeout, mse_fut)
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
    let peer_hs = tokio::time::timeout(hs_timeout, hs_fut)
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
            // Request/piece pipelining is latency-sensitive; Nagle delays
            // small Request messages behind larger writes.
            let _ = tcp.set_nodelay(true);

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
            // Prefer a dedicated ephemeral uTP socket for outgoing dials.
            // Sharing the listener socket with accept/DHT demux yielded 0
            // public-swarm uTP peerconns (Go holds ~25% uTP on Ubuntu);
            // force-uTP on the shared socket never completed handshake.
            // Keep the listener socket as fallback when ephemeral bind fails.
            let (sock, _keep_alive) = {
                let bind_addr: SocketAddr = "0.0.0.0:0"
                    .parse()
                    .map_err(|e| CoreError::Other(format!("invalid addr: {e}")))?;
                match UtpSocket::bind(bind_addr).await {
                    Ok(s) => {
                        let s = Arc::new(s);
                        (s.clone(), Some(s))
                    }
                    Err(_) => {
                        let s = utp_socket.ok_or_else(|| {
                            CoreError::Other("no uTP socket for outgoing dial".into())
                        })?;
                        ((*s).clone(), None)
                    }
                }
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

/// Go `reducedDialTimeout`: shrink nominal timeout as the pending pool grows.
pub fn reduced_dial_timeout(
    min_dial_timeout: Duration,
    nominal: Duration,
    half_open_limit: usize,
    pending_peers: usize,
) -> Duration {
    let limit = half_open_limit.max(1);
    let divisor = pending_peers.saturating_add(limit) / limit;
    (nominal / divisor.max(1) as u32).max(min_dial_timeout)
}

#[cfg(test)]
mod reduced_dial_timeout_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn reduced_dial_timeout_matches_go_vectors() {
        let min = Duration::from_secs(2);
        let nominal = Duration::from_secs(8);
        let half_open = 24;
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 0), nominal);
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 23), nominal);
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 24), Duration::from_secs(4));
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 48), Duration::from_secs(2).max(nominal / 3));
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 4000), min);
    }

    #[test]
    fn reduced_dial_timeout_go_default_config_vectors() {
        let min = Duration::from_secs(3);
        let nominal = Duration::from_secs(20);
        let half_open = 40;
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 0), nominal);
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 39), nominal);
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 40), nominal / 2);
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 80), nominal / 3);
        assert_eq!(reduced_dial_timeout(min, nominal, half_open, 4000), min);
    }
}

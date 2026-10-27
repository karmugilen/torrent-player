//! WebTorrent: WebSocket tracker signaling + WebRTC data channels.
//!
//! Ports `engine-go/patches/torrent/webtorrent/`. ICE/STUN list matches
//! `engine-go/server.go`.

mod protocol;
mod transport;

pub use protocol::{
    binary_to_json_string, json_string_to_bytes, AnnounceRequest as WsAnnounceRequest,
    AnnounceResponse as WsAnnounceResponse, Offer, OfferEnvelope, SdpBlob,
};
pub use transport::{
    default_ice_servers, DataChannelStream, IceServer, PeerConnStats, SessionDescription,
    Transport, DATA_CHANNEL_LABEL, DEFAULT_MAX_OUTSTANDING_OFFERS,
};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{
    client_async_tls_with_config, connect_async_tls_with_config, tungstenite::Message, Connector,
};
use wt_types::InfoHash;

use crate::status::{StatusCallback, TrackerStatusEvent};
use crate::tracker::{AnnounceEvent, DialerHook};

const PING_INTERVAL: Duration = Duration::from_secs(60);
const RECONNECT_DELAY: Duration = Duration::from_secs(60);
const OFFER_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, thiserror::Error)]
pub enum WebTorrentError {
    #[error("websocket: {0}")]
    Ws(String),
    #[error("webrtc: {0}")]
    Webrtc(String),
    #[error("closed")]
    Closed,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, WebTorrentError>;

/// Yielded when a data channel opens (inbound or outbound).
pub struct OpenedPeer {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub offer_id: String,
    pub local_offered: bool,
    pub stream: DataChannelStream,
}

type AnnounceReqFactory =
    Arc<dyn Fn(AnnounceEvent, [u8; 20]) -> crate::tracker::AnnounceRequest + Send + Sync>;

#[derive(Clone)]
pub struct TrackerClientConfig {
    pub url: String,
    pub peer_id: [u8; 20],
    pub ice_servers: Vec<IceServer>,
    pub get_announce_request: Option<AnnounceReqFactory>,
    pub status: Option<StatusCallback>,
    pub max_outstanding_offers: usize,
    pub ws_headers: Vec<(String, String)>,
    /// Custom TCP dialer (Go `trackerSockets.DialContext` for WS trackers).
    pub dialer: Option<DialerHook>,
}

impl Default for TrackerClientConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            peer_id: [0u8; 20],
            ice_servers: default_ice_servers(),
            get_announce_request: None,
            status: None,
            max_outstanding_offers: DEFAULT_MAX_OUTSTANDING_OFFERS,
            ws_headers: Vec::new(),
            dialer: None,
        }
    }
}

struct OutboundOffer {
    info_hash: InfoHash,
    transport: Transport,
    created: Instant,
}

struct Inner {
    cfg: TrackerClientConfig,
    closed: bool,
    outbound: HashMap<String, OutboundOffer>,
    peers_tx: mpsc::UnboundedSender<OpenedPeer>,
    outbox_tx: mpsc::UnboundedSender<String>,
    outbox_rx: Option<mpsc::UnboundedReceiver<String>>,
    stats: PeerConnStats,
}

/// WebSocket tracker client that creates WebRTC offers/answers.
pub struct TrackerClient {
    inner: Arc<Mutex<Inner>>,
    peers_rx: Mutex<Option<mpsc::UnboundedReceiver<OpenedPeer>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl TrackerClient {
    pub fn new(cfg: TrackerClientConfig) -> Arc<Self> {
        let (peers_tx, peers_rx) = mpsc::unbounded_channel();
        let (outbox_tx, outbox_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            inner: Arc::new(Mutex::new(Inner {
                cfg,
                closed: false,
                outbound: HashMap::new(),
                peers_tx,
                outbox_tx,
                outbox_rx: Some(outbox_rx),
                stats: PeerConnStats::default(),
            })),
            peers_rx: Mutex::new(Some(peers_rx)),
            task: Mutex::new(None),
        })
    }

    pub fn take_peers(&self) -> Option<mpsc::UnboundedReceiver<OpenedPeer>> {
        self.peers_rx.lock().take()
    }

    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move { this.run().await });
        *self.task.lock() = Some(handle);
    }

    pub fn close(&self) {
        self.inner.lock().closed = true;
        if let Some(h) = self.task.lock().take() {
            h.abort();
        }
    }

    pub fn stats(&self) -> PeerConnStats {
        self.inner.lock().stats
    }

    /// Announce for an infohash, creating up to `numwant` offers.
    pub async fn announce(&self, info_hash: InfoHash, event: AnnounceEvent, numwant: usize) {
        let (max, ice, peer_id, factory, outbox) = {
            let mut g = self.inner.lock();
            if g.closed {
                return;
            }
            Self::gc_offers(&mut g);
            (
                g.cfg.max_outstanding_offers,
                g.cfg.ice_servers.clone(),
                g.cfg.peer_id,
                g.cfg.get_announce_request.clone(),
                g.outbox_tx.clone(),
            )
        };

        let want = numwant.min(max);
        let mut offers = Vec::new();
        for _ in 0..want {
            let offer_id = random_offer_id();
            match Transport::create_offer(&ice).await {
                Ok((transport, sdp)) => {
                    offers.push(OfferEnvelope::from(Offer {
                        offer_id: offer_id.clone(),
                        sdp_type: sdp.sdp_type.clone(),
                        sdp: sdp.sdp.clone(),
                    }));
                    self.inner.lock().outbound.insert(
                        offer_id,
                        OutboundOffer {
                            info_hash,
                            transport,
                            created: Instant::now(),
                        },
                    );
                }
                Err(e) => tracing::warn!(error = %e, "create offer failed"),
            }
        }

        let ar = if let Some(f) = factory {
            f(event, info_hash.0)
        } else {
            crate::tracker::AnnounceRequest {
                info_hash,
                peer_id,
                event,
                num_want: want as i32,
                ..Default::default()
            }
        };

        let msg = WsAnnounceRequest {
            action: "announce".into(),
            info_hash: binary_to_json_string(&info_hash.0),
            peer_id: binary_to_json_string(&peer_id),
            numwant: want as i32,
            uploaded: ar.uploaded,
            downloaded: ar.downloaded,
            left: ar.left,
            event: match event {
                AnnounceEvent::None => None,
                e => Some(e.as_str().to_string()),
            },
            offers,
        };
        if let Ok(s) = serde_json::to_string(&msg) {
            let _ = outbox.send(s);
        }
    }

    fn gc_offers(g: &mut Inner) {
        let now = Instant::now();
        g.outbound
            .retain(|_, o| now.duration_since(o.created) < OFFER_TTL);
    }

    async fn run(self: Arc<Self>) {
        loop {
            if self.inner.lock().closed {
                break;
            }
            if let Err(e) = self.do_websocket().await {
                tracing::debug!(error = %e, "websocket ended");
                let (cb, url) = {
                    let g = self.inner.lock();
                    (g.cfg.status.clone(), g.cfg.url.clone())
                };
                if let Some(cb) = cb {
                    cb(&TrackerStatusEvent::Disconnected {
                        url,
                        info_hash: InfoHash::default(),
                        error: Some(e.to_string()),
                    });
                }
            }
            if self.inner.lock().closed {
                break;
            }
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }

    async fn do_websocket(self: &Arc<Self>) -> Result<()> {
        let (url, dialer) = {
            let g = self.inner.lock();
            (g.cfg.url.clone(), g.cfg.dialer.clone())
        };
        self.inner.lock().stats.dials += 1;

        let (ws, _) = if let Some(dialer) = dialer {
            connect_ws_with_dialer(&url, dialer).await?
        } else {
            let connector = Connector::Rustls(Arc::new(rustls_client_config()));
            connect_async_tls_with_config(&url, None, false, Some(connector))
                .await
                .map_err(|e| WebTorrentError::Ws(e.to_string()))?
        };
        let (mut sink, mut stream) = ws.split();

        // Re-arm outbox receiver if previous connection took it.
        let mut out_rx = {
            let mut g = self.inner.lock();
            if g.outbox_rx.is_none() {
                let (tx, rx) = mpsc::unbounded_channel();
                g.outbox_tx = tx;
                g.outbox_rx = Some(rx);
            }
            g.outbox_rx.take().expect("outbox rx")
        };

        {
            let (cb, url) = {
                let g = self.inner.lock();
                (g.cfg.status.clone(), g.cfg.url.clone())
            };
            if let Some(cb) = cb {
                cb(&TrackerStatusEvent::Connected {
                    url,
                    info_hash: InfoHash::default(),
                });
            }
        }

        let mut ping = tokio::time::interval(PING_INTERVAL);
        loop {
            if self.inner.lock().closed {
                break;
            }
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(t))) => {
                            self.handle_tracker_message(&t).await?;
                        }
                        Some(Ok(Message::Binary(b))) => {
                            if let Ok(t) = std::str::from_utf8(&b) {
                                self.handle_tracker_message(t).await?;
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            sink.send(Message::Pong(p)).await.map_err(|e| WebTorrentError::Ws(e.to_string()))?;
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(e)) => return Err(WebTorrentError::Ws(e.to_string())),
                        _ => {}
                    }
                }
                Some(text) = out_rx.recv() => {
                    sink.send(Message::Text(text.into())).await.map_err(|e| WebTorrentError::Ws(e.to_string()))?;
                }
                _ = ping.tick() => {
                    sink.send(Message::Ping(Vec::new().into())).await.map_err(|e| WebTorrentError::Ws(e.to_string()))?;
                }
            }
        }
        // Return receiver for next reconnect.
        self.inner.lock().outbox_rx = Some(out_rx);
        Ok(())
    }

    async fn handle_tracker_message(self: &Arc<Self>, text: &str) -> Result<()> {
        let resp: WsAnnounceResponse = match serde_json::from_str(text) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "bad tracker json");
                return Ok(());
            }
        };
        if resp.action != "announce" {
            return Ok(());
        }
        let info_hash = resp
            .info_hash
            .as_deref()
            .and_then(|s| {
                let b = json_string_to_bytes(s).ok()?;
                if b.len() == 20 {
                    let mut a = [0u8; 20];
                    a.copy_from_slice(&b);
                    Some(InfoHash(a))
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if let Some(answer) = resp.answer {
            let offer_id = resp.offer_id.clone().unwrap_or_default();
            let (outbound, peers_tx, peer_id, status, url) = {
                let mut g = self.inner.lock();
                let outbound = g.outbound.remove(&offer_id);
                let peer_id = resp
                    .peer_id
                    .as_deref()
                    .and_then(|s| {
                        let b = json_string_to_bytes(s).ok()?;
                        (b.len() == 20).then(|| {
                            let mut a = [0u8; 20];
                            a.copy_from_slice(&b);
                            a
                        })
                    })
                    .unwrap_or([0u8; 20]);
                if outbound.is_some() {
                    g.stats.converted_outbound_conns += 1;
                }
                (
                    outbound,
                    g.peers_tx.clone(),
                    peer_id,
                    g.cfg.status.clone(),
                    g.cfg.url.clone(),
                )
            };
            if let Some(mut outbound) = outbound {
                match outbound
                    .transport
                    .accept_answer(&answer.sdp_type, &answer.sdp)
                    .await
                {
                    Ok(stream) => {
                        let _ = peers_tx.send(OpenedPeer {
                            info_hash: outbound.info_hash,
                            peer_id,
                            offer_id,
                            local_offered: true,
                            stream,
                        });
                        if let Some(cb) = status {
                            cb(&TrackerStatusEvent::AnnounceSuccessful { url, info_hash });
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "accept answer failed"),
                }
            }
            return Ok(());
        }

        if let Some(offer) = resp.offer {
            let offer_id = resp.offer_id.clone().unwrap_or_default();
            let (ice, to_peer, peer_id, outbox, peers_tx) = {
                let g = self.inner.lock();
                (
                    g.cfg.ice_servers.clone(),
                    resp.to_peer_id.clone(),
                    g.cfg.peer_id,
                    g.outbox_tx.clone(),
                    g.peers_tx.clone(),
                )
            };
            match Transport::create_answer(&ice, &offer.sdp_type, &offer.sdp).await {
                Ok((transport, answer_sdp, stream_rx)) => {
                    let answer_msg = serde_json::json!({
                        "action": "announce",
                        "info_hash": binary_to_json_string(&info_hash.0),
                        "peer_id": binary_to_json_string(&peer_id),
                        "to_peer_id": to_peer,
                        "offer_id": offer_id,
                        "answer": {
                            "type": answer_sdp.sdp_type,
                            "sdp": answer_sdp.sdp,
                        }
                    });
                    if let Ok(s) = serde_json::to_string(&answer_msg) {
                        let _ = outbox.send(s);
                    }
                    self.inner.lock().stats.converted_inbound_conns += 1;
                    let peer_id_remote = resp.peer_id.clone();
                    tokio::spawn(async move {
                        // Keep transport alive until channel opens.
                        let _transport = transport;
                        if let Ok(stream) = stream_rx.await {
                            let peer_bytes = peer_id_remote
                                .as_deref()
                                .and_then(|s| json_string_to_bytes(s).ok())
                                .and_then(|b| {
                                    (b.len() == 20).then(|| {
                                        let mut a = [0u8; 20];
                                        a.copy_from_slice(&b);
                                        a
                                    })
                                })
                                .unwrap_or([0u8; 20]);
                            let _ = peers_tx.send(OpenedPeer {
                                info_hash,
                                peer_id: peer_bytes,
                                offer_id,
                                local_offered: false,
                                stream,
                            });
                        }
                    });
                }
                Err(e) => tracing::warn!(error = %e, "create answer failed"),
            }
        }
        Ok(())
    }
}

fn random_offer_id() -> String {
    let mut b = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut b);
    binary_to_json_string(&b)
}

fn rustls_client_config() -> rustls::ClientConfig {
    crate::ensure_rustls_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}

async fn connect_ws_with_dialer(
    url: &str,
    dialer: DialerHook,
) -> Result<(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
)> {
    let parsed = url::Url::parse(url).map_err(|e| WebTorrentError::Ws(e.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| WebTorrentError::Ws("ws url missing host".into()))?;
    let secure = parsed.scheme() == "wss";
    let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
    let addr = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let tcp = dialer("tcp".into(), addr)
        .await
        .map_err(|e| WebTorrentError::Ws(e.to_string()))?;
    let request = url
        .into_client_request()
        .map_err(|e| WebTorrentError::Ws(e.to_string()))?;
    let connector = Connector::Rustls(Arc::new(rustls_client_config()));
    client_async_tls_with_config(request, tcp, None, Some(connector))
        .await
        .map_err(|e| WebTorrentError::Ws(e.to_string()))
}

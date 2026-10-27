//! # Peer sources glue (agy-2 integration guide)
//!
//! Drives per-torrent discovery on top of `wt-discovery`: HTTP/UDP trackers
//! ([`AnnounceDispatcher`]), DHT (dedicated or shared [`DatagramIo`]), and
//! WebTorrent WebSocket/WebRTC trackers. Ports the discovery half of
//! `engine-go/patches/torrent/{client.go,torrent.go,client-tracker-announcer.go,dht.go}`
//! plus the hybrid-magnet fix in `WEBTOR_PATCH.md`.
//!
//! ## What agy-2 should call
//!
//! Build once per [`crate::Client`] (after listen / shared UDP are ready):
//!
//! ```ignore
//! use wt_core::sources::{Sources, SourcesConfig, TorrentKeys, TorrentAnnounceInfo, PeerSink};
//! use wt_core::conn::SharedDatagramAdapter; // when conn is wired
//!
//! let mut sc = SourcesConfig::from_client_config(&client.config());
//! sc.peer_id = client.peer_id();
//! sc.datagram = Some(Arc::new(SharedDatagramAdapter::new(utp_sock))); // or None
//! sc.dialer = tracker_dialer; // Option<DialerHook>
//! let sources = Sources::start(sc).await?;
//! ```
//!
//! On `add_torrent_spec` / drop / pause / `add_trackers` / stats change:
//!
//! ```ignore
//! let keys = TorrentKeys::from_spec(&spec); // registers BOTH v1 and v2.to_short()
//! sources.register_torrent(RegisterTorrent {
//!     keys: keys.clone(),
//!     trackers: spec.trackers.clone(),
//!     web_seeds: spec.web_seeds.clone(),
//!     info: torrent_announce_info, // Arc<dyn TorrentAnnounceInfo>
//!     sink: torrent_peer_sink,     // Arc<dyn PeerSink>
//! })?;
//! sources.refresh_torrent(&keys);           // after downloaded/left/want_peers change
//! sources.add_trackers(&keys, tiers)?;      // no-op for unregistered short hashes
//! sources.set_paused(&keys, true);          // Stopped + stop DHT/WS
//! sources.unregister_torrent(&keys);        // Drop: Stopped then forget
//! let contacts = sources.dht_contacts().await;
//! sources.add_dht_nodes(&["1.2.3.4:6881".into()]);
//! sources.close().await;
//! ```
//!
//! Implement [`TorrentAnnounceInfo`] on the torrent (stats + `want_peers` from
//! peer low/high water) and [`PeerSink`] to feed `Torrent::add_peers` /
//! WebRTC streams. Status events are mapped to [`crate::StatusEvent`] and
//! delivered through `SourcesConfig::status_callbacks` (wire
//! `ClientConfig.callbacks.status_updated`).

mod status_map;
mod traits;

pub use status_map::map_tracker_status;
pub use traits::{PeerSink, RegisterTorrent, TorrentAnnounceInfo, TorrentKeys, WebRtcOpenedPeer};

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wt_discovery::dht::{Contact, DhtConfig, DhtNode, DEFAULT_BOOTSTRAP_NODES};
use wt_discovery::tracker::{
    AnnounceDispatcher, AnnounceDispatcherConfig, AnnounceOpt, ClientOpts, DialerHook,
    HttpClientOpts, Peer as TrackerPeer,
};
use wt_discovery::webtorrent::{IceServer, TrackerClient, TrackerClientConfig};
use wt_discovery::{DatagramIo, TrackerStatusEvent};
use wt_types::{InfoHash, PeerId, PeerInfo, PeerSource};

use crate::{ClientConfig, CoreError, Result, StatusEvent, TorrentSpec};

/// Default DHT re-announce interval (Go `dhtAnnouncer` cadence is driven by
/// the DHT itself; we refresh get_peers/announce on this period when enabled).
pub const DEFAULT_DHT_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Status callback list (same shape as `ClientConfig.callbacks.status_updated`).
pub type StatusCallbacks = Vec<Arc<dyn Fn(&StatusEvent) + Send + Sync>>;

/// Settings derived from [`ClientConfig`] plus discovery-specific hooks.
#[derive(Clone)]
pub struct SourcesConfig {
    pub no_dht: bool,
    pub disable_webtorrent: bool,
    pub periodically_announce_torrents_to_dht: bool,
    pub http_user_agent: String,
    pub listen_port: u16,
    pub peer_id: PeerId,
    pub ice_servers: Vec<String>,
    pub torrent_peers_low_water: usize,
    pub torrent_peers_high_water: usize,
    pub dht_announce_interval: Duration,
    /// Shared UDP from agy-1 (`SharedDatagramAdapter`); `None` = dedicated DHT socket.
    pub datagram: Option<Arc<dyn DatagramIo>>,
    /// Go `trackerSockets.DialContext` for HTTP(S)/WS trackers.
    pub dialer: Option<DialerHook>,
    /// Usually `ClientConfig.callbacks.status_updated`.
    pub status_callbacks: StatusCallbacks,
}

impl SourcesConfig {
    pub fn from_client_config(cfg: &ClientConfig) -> Self {
        Self {
            no_dht: cfg.no_dht,
            disable_webtorrent: cfg.disable_webtorrent,
            periodically_announce_torrents_to_dht: cfg.periodically_announce_torrents_to_dht,
            http_user_agent: cfg.http_user_agent.clone(),
            listen_port: cfg.listen_port,
            peer_id: [0u8; 20],
            ice_servers: cfg.ice_servers.clone(),
            torrent_peers_low_water: cfg.torrent_peers_low_water,
            torrent_peers_high_water: cfg.torrent_peers_high_water,
            dht_announce_interval: DEFAULT_DHT_ANNOUNCE_INTERVAL,
            datagram: None,
            dialer: None,
            status_callbacks: cfg.callbacks.status_updated.clone(),
        }
    }
}

struct TorrentEntry {
    keys: TorrentKeys,
    info: Arc<dyn TorrentAnnounceInfo>,
    sink: Arc<dyn PeerSink>,
    web_seeds: Vec<String>,
    paused: bool,
    /// WebSocket tracker clients keyed by URL.
    ws_clients: Vec<Arc<TrackerClient>>,
}

struct Inner {
    cfg: SourcesConfig,
    /// Registered short infohashes (hybrid: both v1 and v2 short).
    registered: HashSet<InfoHash>,
    torrents: HashMap<InfoHash, Arc<Mutex<TorrentEntry>>>,
    /// Canonical entry per torrent identity (first short hash).
    by_identity: HashMap<u64, Arc<Mutex<TorrentEntry>>>,
    next_identity: u64,
    closed: bool,
}

/// Client-level discovery driver.
pub struct Sources {
    inner: Arc<Mutex<Inner>>,
    dispatcher: Arc<AnnounceDispatcher>,
    dht: Option<Arc<DhtNode>>,
    peers_rx_task: Mutex<Option<JoinHandle<()>>>,
    dht_periodic: Mutex<Option<JoinHandle<()>>>,
    ws_peer_tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Sources {
    /// Start announcer (+ optional DHT). Call from an async runtime.
    pub async fn start(cfg: SourcesConfig) -> Result<Arc<Self>> {
        wt_discovery::ensure_rustls_provider();

        let status_cbs = cfg.status_callbacks.clone();
        let status: wt_discovery::StatusCallback = Arc::new(move |ev: &TrackerStatusEvent| {
            let mapped = map_tracker_status(ev);
            for cb in &status_cbs {
                cb(&mapped);
            }
        });

        let (peers_tx, peers_rx) = mpsc::unbounded_channel::<(InfoHash, Vec<TrackerPeer>)>();

        let client_opts = ClientOpts {
            http: HttpClientOpts {
                dialer: cfg.dialer.clone(),
                ..Default::default()
            },
            dialer: cfg.dialer.clone(),
            datagram: cfg.datagram.clone(),
            ..Default::default()
        };

        let dispatcher = AnnounceDispatcher::new(AnnounceDispatcherConfig {
            announce_opt: AnnounceOpt {
                user_agent: cfg.http_user_agent.clone(),
                ..Default::default()
            },
            client_opts,
            status: Some(status),
            peers_tx: Some(peers_tx),
        });

        let dht = if cfg.no_dht {
            None
        } else {
            let dht_cfg = DhtConfig {
                port: cfg.listen_port,
                bootstrap: DEFAULT_BOOTSTRAP_NODES
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                ..Default::default()
            };
            let node = if let Some(io) = cfg.datagram.clone() {
                DhtNode::with_datagram_io(io, dht_cfg)
                    .await
                    .map_err(|e| CoreError::Other(e.to_string()))?
            } else {
                DhtNode::new(dht_cfg)
                    .await
                    .map_err(|e| CoreError::Other(e.to_string()))?
            };
            Some(Arc::new(node))
        };

        let me = Arc::new(Self {
            inner: Arc::new(Mutex::new(Inner {
                cfg: cfg.clone(),
                registered: HashSet::new(),
                torrents: HashMap::new(),
                by_identity: HashMap::new(),
                next_identity: 1,
                closed: false,
            })),
            dispatcher,
            dht,
            peers_rx_task: Mutex::new(None),
            dht_periodic: Mutex::new(None),
            ws_peer_tasks: Mutex::new(Vec::new()),
        });

        let run = Arc::clone(&me);
        let handle = tokio::spawn(async move { run.peers_loop(peers_rx).await });
        *me.peers_rx_task.lock() = Some(handle);

        if cfg.periodically_announce_torrents_to_dht && me.dht.is_some() {
            let run = Arc::clone(&me);
            let interval = cfg.dht_announce_interval;
            let handle = tokio::spawn(async move { run.dht_periodic_loop(interval).await });
            *me.dht_periodic.lock() = Some(handle);
        }

        Ok(me)
    }

    /// Register a torrent for discovery under all short hashes in `keys`.
    ///
    /// Hybrid magnets must pass both v1 and `InfoHashV2::to_short()` (see
    /// [`TorrentKeys::from_spec`]). Trackers are attached only for registered
    /// keys — an unregistered short hash gets no announcer (WEBTOR_PATCH).
    pub fn register_torrent(&self, reg: RegisterTorrent) -> Result<()> {
        let mut g = self.inner.lock();
        if g.closed {
            return Err(CoreError::Closed);
        }
        let shorts = reg.keys.all_short();
        if shorts.is_empty() {
            return Err(CoreError::Other("register_torrent: no infohashes".into()));
        }
        for ih in &shorts {
            if g.registered.contains(ih) {
                return Err(CoreError::Other(format!(
                    "infohash {} already registered",
                    ih.to_hex()
                )));
            }
        }

        let identity = g.next_identity;
        g.next_identity += 1;

        let (http_udp, ws_urls) = split_tracker_urls(&reg.trackers);

        let entry = Arc::new(Mutex::new(TorrentEntry {
            keys: reg.keys.clone(),
            info: Arc::clone(&reg.info),
            sink: Arc::clone(&reg.sink),
            web_seeds: reg.web_seeds.clone(),
            paused: false,
            ws_clients: Vec::new(),
        }));

        for ih in &shorts {
            g.registered.insert(*ih);
            g.torrents.insert(*ih, Arc::clone(&entry));
        }
        g.by_identity.insert(identity, Arc::clone(&entry));

        let peer_id = reg.info.peer_id();
        let port = reg.info.port().max(g.cfg.listen_port);
        drop(g);

        // Snapshots + tracker slots for every registered short hash.
        for ih in &shorts {
            self.push_snapshot(*ih, &reg.info, port);
            self.dispatcher.add_trackers(*ih, http_udp.clone());
        }

        // Web seeds → sink (grok-6 / webseed_peer dials).
        if !reg.web_seeds.is_empty() {
            entry.lock().web_seeds = reg.web_seeds.clone();
            reg.sink.add_web_seeds(shorts[0], reg.web_seeds.clone());
        }

        // WebTorrent WS trackers.
        if !self.inner.lock().cfg.disable_webtorrent {
            for url in ws_urls {
                self.start_ws_tracker(&reg.keys, &url, peer_id, Arc::clone(&reg.sink));
            }
        }

        // Kick DHT announce for this torrent when enabled.
        if let Some(dht) = &self.dht {
            let dht = Arc::clone(dht);
            let sink = Arc::clone(&reg.sink);
            let keys = reg.keys.clone();
            let port = reg.info.port().max(self.inner.lock().cfg.listen_port);
            tokio::spawn(async move {
                announce_to_dht(&dht, &keys, port, &sink, PeerSource::DhtGetPeers).await;
            });
        }

        Ok(())
    }

    /// Refresh announce snapshots after stats / want_peers change.
    pub fn refresh_torrent(&self, keys: &TorrentKeys) {
        let g = self.inner.lock();
        if g.closed {
            return;
        }
        let Some(entry) = keys
            .all_short()
            .into_iter()
            .find_map(|ih| g.torrents.get(&ih).cloned())
        else {
            return;
        };
        let port = g.cfg.listen_port;
        drop(g);
        let e = entry.lock();
        if e.paused {
            return;
        }
        let port = e.info.port().max(port);
        for ih in e.keys.all_short() {
            if self.inner.lock().registered.contains(&ih) {
                self.push_snapshot(ih, &e.info, port);
            }
        }
    }

    /// Add tracker tiers. URLs for short hashes that are not registered are ignored.
    pub fn add_trackers(&self, keys: &TorrentKeys, tiers: Vec<Vec<String>>) -> Result<()> {
        let g = self.inner.lock();
        if g.closed {
            return Err(CoreError::Closed);
        }
        let (http_udp, ws_urls) = split_tracker_urls(&tiers);
        let shorts: Vec<InfoHash> = keys
            .all_short()
            .into_iter()
            .filter(|ih| g.registered.contains(ih))
            .collect();
        if shorts.is_empty() {
            // Hybrid fix: do not create announcers for unregistered keys.
            return Ok(());
        }
        let entry = g.torrents.get(&shorts[0]).cloned();
        let disable_ws = g.cfg.disable_webtorrent;
        let peer_id = entry
            .as_ref()
            .map(|e| e.lock().info.peer_id())
            .unwrap_or([0u8; 20]);
        let sink = entry.as_ref().map(|e| Arc::clone(&e.lock().sink));
        drop(g);

        for ih in &shorts {
            self.dispatcher.add_trackers(*ih, http_udp.clone());
        }
        if !disable_ws {
            if let Some(sink) = sink {
                for url in ws_urls {
                    self.start_ws_tracker(keys, &url, peer_id, sink.clone());
                }
            }
        }
        Ok(())
    }

    pub fn set_paused(&self, keys: &TorrentKeys, paused: bool) {
        let g = self.inner.lock();
        let Some(entry) = keys
            .all_short()
            .into_iter()
            .find_map(|ih| g.torrents.get(&ih).cloned())
        else {
            return;
        };
        drop(g);
        let mut e = entry.lock();
        e.paused = paused;
        let shorts = e.keys.all_short();
        if paused {
            for ih in &shorts {
                self.dispatcher.remove_torrent(*ih);
            }
            for c in e.ws_clients.drain(..) {
                c.close();
            }
        } else {
            let port = e.info.port().max(self.inner.lock().cfg.listen_port);
            for ih in &shorts {
                self.push_snapshot(*ih, &e.info, port);
            }
        }
    }

    pub fn unregister_torrent(&self, keys: &TorrentKeys) {
        let mut g = self.inner.lock();
        let shorts = keys.all_short();
        let entry = shorts.iter().find_map(|ih| g.torrents.get(ih).cloned());
        for ih in &shorts {
            g.registered.remove(ih);
            g.torrents.remove(ih);
        }
        if let Some(ref ent) = entry {
            g.by_identity.retain(|_, e| !Arc::ptr_eq(e, ent));
        }
        drop(g);

        for ih in &shorts {
            self.dispatcher.remove_torrent(*ih);
        }
        if let Some(entry) = entry {
            let mut e = entry.lock();
            for c in e.ws_clients.drain(..) {
                c.close();
            }
        }
    }

    pub fn add_dht_nodes(&self, nodes: &[String]) {
        let Some(dht) = &self.dht else {
            return;
        };
        let dht = Arc::clone(dht);
        let nodes: Vec<String> = nodes.to_vec();
        tokio::spawn(async move {
            for n in nodes {
                let addr: SocketAddr = match parse_host_port(&n) {
                    Some(a) => a,
                    None => continue,
                };
                dht.add_node(Contact {
                    id: [0u8; 20],
                    addr,
                })
                .await;
            }
        });
    }

    pub async fn dht_contacts(&self) -> Vec<SocketAddr> {
        let Some(dht) = &self.dht else {
            return Vec::new();
        };
        dht.export_contacts()
            .await
            .into_iter()
            .map(|c| c.addr)
            .collect()
    }

    pub fn dht_node(&self) -> Option<Arc<DhtNode>> {
        self.dht.clone()
    }

    /// True when a short infohash has been registered (hybrid tests).
    pub fn is_registered(&self, ih: &InfoHash) -> bool {
        self.inner.lock().registered.contains(ih)
    }

    pub async fn close(&self) {
        {
            let mut g = self.inner.lock();
            g.closed = true;
            let entries: Vec<_> = g.by_identity.values().cloned().collect();
            g.registered.clear();
            g.torrents.clear();
            g.by_identity.clear();
            drop(g);
            for e in entries {
                let mut ent = e.lock();
                for ih in ent.keys.all_short() {
                    self.dispatcher.remove_torrent(ih);
                }
                for c in ent.ws_clients.drain(..) {
                    c.close();
                }
            }
        }
        self.dispatcher.close();
        if let Some(dht) = &self.dht {
            dht.close();
        }
        if let Some(h) = self.peers_rx_task.lock().take() {
            h.abort();
        }
        if let Some(h) = self.dht_periodic.lock().take() {
            h.abort();
        }
        for h in self.ws_peer_tasks.lock().drain(..) {
            h.abort();
        }
    }

    fn push_snapshot(&self, ih: InfoHash, info: &Arc<dyn TorrentAnnounceInfo>, port: u16) {
        self.dispatcher
            .update_torrent(wt_discovery::tracker::TorrentAnnounceSnapshot {
                info_hash: ih,
                peer_id: info.peer_id(),
                downloaded: info.downloaded(),
                left: info.left(),
                uploaded: info.uploaded(),
                port,
                key: info.announce_key(),
                need_data: info.need_data(),
                want_peers: info.want_peers(),
                completed: info.completed(),
                newly_completed: info.newly_completed(),
            });
    }

    async fn peers_loop(
        self: Arc<Self>,
        mut rx: mpsc::UnboundedReceiver<(InfoHash, Vec<TrackerPeer>)>,
    ) {
        while let Some((ih, peers)) = rx.recv().await {
            let sink = {
                let g = self.inner.lock();
                g.torrents.get(&ih).map(|e| Arc::clone(&e.lock().sink))
            };
            let Some(sink) = sink else {
                continue;
            };
            let mapped: Vec<PeerInfo> = peers
                .into_iter()
                .map(|p| PeerInfo {
                    id: p.id,
                    addr: p.addr,
                    source: PeerSource::Tracker,
                    supports_encryption: false,
                    pex: Default::default(),
                    trusted: false,
                })
                .collect();
            if !mapped.is_empty() {
                sink.add_peers(ih, mapped);
            }
        }
    }

    async fn dht_periodic_loop(self: Arc<Self>, interval: Duration) {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            if self.inner.lock().closed {
                break;
            }
            let Some(dht) = self.dht.clone() else {
                break;
            };
            let batch: Vec<(TorrentKeys, Arc<dyn PeerSink>, u16)> = {
                let g = self.inner.lock();
                g.by_identity
                    .values()
                    .filter_map(|e| {
                        let ent = e.lock();
                        if ent.paused {
                            return None;
                        }
                        Some((
                            ent.keys.clone(),
                            Arc::clone(&ent.sink),
                            ent.info.port().max(g.cfg.listen_port),
                        ))
                    })
                    .collect()
            };
            for (keys, sink, port) in batch {
                announce_to_dht(&dht, &keys, port, &sink, PeerSource::DhtGetPeers).await;
            }
        }
    }

    fn start_ws_tracker(
        &self,
        keys: &TorrentKeys,
        url: &str,
        peer_id: PeerId,
        sink: Arc<dyn PeerSink>,
    ) {
        let ice: Vec<IceServer> = {
            let g = self.inner.lock();
            if g.cfg.ice_servers.is_empty() {
                wt_discovery::webtorrent::default_ice_servers()
            } else {
                vec![IceServer {
                    urls: g.cfg.ice_servers.clone(),
                }]
            }
        };
        let dialer = self.inner.lock().cfg.dialer.clone();
        let status_cbs = self.inner.lock().cfg.status_callbacks.clone();
        let status: wt_discovery::StatusCallback = Arc::new(move |ev: &TrackerStatusEvent| {
            let mapped = map_tracker_status(ev);
            for cb in &status_cbs {
                cb(&mapped);
            }
        });

        let primary = keys.all_short().into_iter().next().unwrap_or_default();

        let client = TrackerClient::new(TrackerClientConfig {
            url: url.to_string(),
            peer_id,
            ice_servers: ice,
            status: Some(status),
            dialer,
            ..Default::default()
        });
        let mut peers_rx = client.take_peers().expect("peers rx");
        client.start();

        // Store on entry.
        if let Some(entry) = self.inner.lock().torrents.get(&primary).cloned() {
            entry.lock().ws_clients.push(Arc::clone(&client));
        }

        let info_hashes = keys.all_short();
        for ih in &info_hashes {
            let c = Arc::clone(&client);
            let ih = *ih;
            tokio::spawn(async move {
                c.announce(ih, wt_discovery::tracker::AnnounceEvent::Started, 1)
                    .await;
            });
        }

        let sink2 = Arc::clone(&sink);
        let handle = tokio::spawn(async move {
            while let Some(opened) = peers_rx.recv().await {
                sink2.add_webrtc_peer(
                    opened.info_hash,
                    WebRtcOpenedPeer {
                        info_hash: opened.info_hash,
                        peer_id: opened.peer_id,
                        offer_id: opened.offer_id,
                        local_offered: opened.local_offered,
                        stream: opened.stream,
                    },
                );
            }
        });
        self.ws_peer_tasks.lock().push(handle);
    }
}

async fn announce_to_dht(
    dht: &DhtNode,
    keys: &TorrentKeys,
    port: u16,
    sink: &Arc<dyn PeerSink>,
    source: PeerSource,
) {
    for ih in keys.all_short() {
        let Ok(mut handle) = dht.announce(ih, port, true).await else {
            continue;
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(2), handle.next()).await {
                Ok(Some(vals)) => {
                    let peers: Vec<PeerInfo> = vals
                        .peers
                        .into_iter()
                        .filter(|a| a.port() != 0)
                        .map(|addr| PeerInfo {
                            id: None,
                            addr,
                            source,
                            supports_encryption: false,
                            pex: Default::default(),
                            trusted: false,
                        })
                        .collect();
                    if !peers.is_empty() {
                        sink.add_peers(ih, peers);
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        handle.close();
    }
}

fn split_tracker_urls(tiers: &[Vec<String>]) -> (Vec<String>, Vec<String>) {
    let mut http_udp = Vec::new();
    let mut ws = Vec::new();
    for tier in tiers {
        for url in tier {
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("ws://") || lower.starts_with("wss://") {
                ws.push(url.clone());
            } else if lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("udp://")
                || lower.starts_with("udp4://")
                || lower.starts_with("udp6://")
            {
                http_udp.push(url.clone());
            }
        }
    }
    (http_udp, ws)
}

fn parse_host_port(s: &str) -> Option<SocketAddr> {
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Some(a);
    }
    // host:port without brackets
    let (host, port_s) = s.rsplit_once(':')?;
    let port: u16 = port_s.parse().ok()?;
    let ip: std::net::IpAddr = host.parse().ok()?;
    Some(SocketAddr::new(ip, port))
}

impl TorrentKeys {
    pub fn from_spec(spec: &TorrentSpec) -> Self {
        Self {
            v1: spec.info_hash,
            v2_short: spec.info_hash_v2.as_ref().map(|v| v.to_short()),
        }
    }
}

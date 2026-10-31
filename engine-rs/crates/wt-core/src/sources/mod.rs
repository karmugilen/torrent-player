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

/// Fallback poll when a torrent does not currently want connections (Go waits
/// on `cl.event`; we poll so want_conns flips wake us without a condvar).
pub const DHT_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Go `dhtAnnouncer` sleeps 5 minutes after an announce error (DNS etc.).
pub const DHT_ANNOUNCE_ERROR_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Go `timeboxedAnnounceToDht` keeps one get_peers traversal open this long.
pub const DHT_ANNOUNCE_TIMEBOX: Duration = Duration::from_secs(15 * 60);
/// Kept for SourcesConfig / callers; continuous announcer supersedes the old
/// 15-minute timer while `want_conns` is true (Go `dhtAnnouncer`).
pub const DEFAULT_DHT_ANNOUNCE_INTERVAL: Duration = DHT_ANNOUNCE_TIMEBOX;

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
    /// Per-torrent Go-style `dhtAnnouncer` task (aborted on unregister/pause).
    dht_announcer: Option<JoinHandle<()>>,
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
            // The mainline DHT always binds its own UDP socket. When uTP already
            // holds the listen port (cfg.datagram is set), binding that port
            // again fails with EADDRINUSE, so the DHT takes an ephemeral port
            // and announces the listen port explicitly (see announce_to_dht).
            let port = if cfg.datagram.is_some() {
                0
            } else {
                cfg.listen_port
            };
            let dht_cfg = DhtConfig {
                port,
                bootstrap: DEFAULT_BOOTSTRAP_NODES
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                ..Default::default()
            };
            // Like Go, a DHT failure is not fatal: trackers keep working.
            match DhtNode::new(dht_cfg).await {
                Ok(node) => Some(Arc::new(node)),
                Err(e) => {
                    tracing::warn!("dht disabled: {e}");
                    None
                }
            }
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

        // Per-torrent continuous DHT announcers are spawned in register_torrent
        // (Go dhtAnnouncer). The old client-wide 15-minute timer is unused when
        // periodically_announce_torrents_to_dht is true; keep the flag as the
        // gate for spawning those tasks.
        let _ = cfg.periodically_announce_torrents_to_dht;

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
            dht_announcer: None,
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

        // Go `dhtAnnouncer`: keep get_peers/announce running while want_conns.
        self.spawn_dht_announcer(Arc::clone(&entry));

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
        // Lock order: `inner` and an entry are never held together.
        // add_trackers takes inner → entry; holding the entry while taking
        // inner here deadlocked torrent add against the dial loop.
        let (keys, info) = {
            let e = entry.lock();
            if e.paused {
                return;
            }
            (e.keys.clone(), Arc::clone(&e.info))
        };
        let port = info.port().max(port);
        for ih in keys.all_short() {
            if self.inner.lock().registered.contains(&ih) {
                self.push_snapshot(ih, &info, port);
            }
        }
    }

    /// Go network recovery: announce to every tracker now and restart the DHT
    /// announcer, so the new route finds peers without waiting out intervals.
    pub fn rediscover(&self, keys: &TorrentKeys) {
        let g = self.inner.lock();
        if g.closed {
            return;
        }
        let Some(entry) = keys.all_short().into_iter().find_map(|ih| g.torrents.get(&ih).cloned())
        else {
            return;
        };
        let port = g.cfg.listen_port;
        drop(g);
        {
            let e = entry.lock();
            if e.paused {
                return;
            }
            let port = e.info.port().max(port);
            for ih in e.keys.all_short() {
                self.push_snapshot(ih, &e.info, port);
                self.dispatcher.announce_now(ih);
            }
        }
        self.spawn_dht_announcer(entry);
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
        // Release `inner` before locking the entry (see refresh_torrent).
        drop(g);
        let peer_id = entry
            .as_ref()
            .map(|e| e.lock().info.peer_id())
            .unwrap_or([0u8; 20]);
        let sink = entry.as_ref().map(|e| Arc::clone(&e.lock().sink));

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
        let listen_port = g.cfg.listen_port;
        // Release `inner` before the entry; never take it back while the
        // entry is held (see refresh_torrent).
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
            if let Some(h) = e.dht_announcer.take() {
                h.abort();
            }
        } else {
            let port = e.info.port().max(listen_port);
            for ih in &shorts {
                self.push_snapshot(*ih, &e.info, port);
            }
            drop(e);
            self.spawn_dht_announcer(entry);
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
            if let Some(h) = e.dht_announcer.take() {
                h.abort();
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
                if let Some(h) = ent.dht_announcer.take() {
                    h.abort();
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
                    // Go AppendFromTracker leaves Trusted=false; marking tracker
                    // peers trusted made them monopolize PopMax ahead of dialable
                    // DHT contacts and burned half-open on firewalled timeouts.
                    trusted: false,
                })
                .collect();
            if !mapped.is_empty() {
                sink.add_peers(ih, mapped);
            }
        }
    }

    /// Spawn Go `dhtAnnouncer`: continuous get_peers while `want_conns`.
    fn spawn_dht_announcer(&self, entry: Arc<Mutex<TorrentEntry>>) {
        let enabled = {
            let g = self.inner.lock();
            g.cfg.periodically_announce_torrents_to_dht && !g.closed
        };
        let Some(dht) = self.dht.clone() else {
            return;
        };
        if !enabled {
            return;
        }

        {
            let mut e = entry.lock();
            if e.paused {
                return;
            }
            if let Some(h) = e.dht_announcer.take() {
                h.abort();
            }
        }

        let sources = Arc::clone(&self.inner);
        let entry_task = Arc::clone(&entry);
        let handle = tokio::spawn(async move {
            loop {
                let listen_port = {
                    let g = sources.lock();
                    if g.closed {
                        break;
                    }
                    g.cfg.listen_port
                };
                let (keys, sink, port, want, paused) = {
                    let e = entry_task.lock();
                    (
                        e.keys.clone(),
                        Arc::clone(&e.sink),
                        e.info.port().max(listen_port),
                        // BEP 27: private torrents never use the DHT.
                        e.info.want_conns() && !e.info.private(),
                        e.paused,
                    )
                };
                if paused {
                    break;
                }
                if !want {
                    tokio::time::sleep(DHT_IDLE_POLL_INTERVAL).await;
                    continue;
                }
                // Go `timeboxedAnnounceToDht`: one traversal up to 15m, then
                // immediately again while want_conns.
                let ok = announce_to_dht(&dht, &keys, port, &sink, PeerSource::DhtGetPeers).await;
                if !ok {
                    tracing::warn!(
                        "dht announce error; backing off {:?}",
                        DHT_ANNOUNCE_ERROR_BACKOFF
                    );
                    tokio::time::sleep(DHT_ANNOUNCE_ERROR_BACKOFF).await;
                    continue;
                }
                // Brief yield so a stuck announce cannot spin the runtime.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        entry.lock().dht_announcer = Some(handle);
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
            status_info_hash: primary,
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
                // Wait briefly for the WebSocket to come up so the first
                // announce is not lost on a reconnect that replaces the outbox.
                tokio::time::sleep(Duration::from_millis(500)).await;
                // Go WebTorrent trackers request multiple offers; 1 was starving
                // the WebRTC peer pool on public magnets.
                c.announce(ih, wt_discovery::tracker::AnnounceEvent::Started, 10)
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
        let mut tasks = self.ws_peer_tasks.lock();
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }
}

/// Returns false when every short-hash announce failed to start (Go backs off).
async fn announce_to_dht(
    dht: &DhtNode,
    keys: &TorrentKeys,
    port: u16,
    sink: &Arc<dyn PeerSink>,
    source: PeerSource,
) -> bool {
    let mut any_ok = false;
    let mut total_peers = 0usize;
    for ih in keys.all_short() {
        // Explicit port: the DHT socket's port is not the peer listen port.
        let mut handle = match dht.announce(ih, port, false).await {
            Ok(h) => {
                any_ok = true;
                h
            }
            Err(e) => {
                tracing::debug!("dht announce start failed for {}: {e}", ih.to_hex());
                continue;
            }
        };
        // Go `timeboxedAnnounceToDht`: keep consuming PeersValues until the
        // stream ends or 15m elapses. A short idle abort starved unique addrs
        // mid-run once the first nearby DHT nodes had been queried.
        let deadline = tokio::time::Instant::now() + DHT_ANNOUNCE_TIMEBOX;
        while tokio::time::Instant::now() < deadline {
            let wait = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(wait, handle.next()).await {
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
                        total_peers += peers.len();
                        tracing::debug!(
                            "dht get_peers +{} for {} (batch)",
                            peers.len(),
                            ih.to_hex()
                        );
                        sink.add_peers(ih, peers);
                    }
                }
                Ok(None) => break,
                Err(_) => break, // timebox elapsed
            }
        }
        handle.close();
    }
    tracing::debug!("dht announce finished: any_ok={any_ok} peers={total_peers}");
    any_ok
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

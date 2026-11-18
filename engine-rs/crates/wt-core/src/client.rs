//! Client runtime, peer networking listener & dialer, and swarm coordinator.
//!
//! Ports the Client side of `engine-go/patches/torrent/client.go`: one Tokio
//! runtime, hybrid short-hash registry, PeerListener (TCP+uTP), half-open /
//! dial limiters, Sources (DHT/trackers/WebTorrent), and peer connection drivers.

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use wt_types::{InfoHash, InfoHashV2, PeerId, PeerInfo, PeerSource};

use crate::conn::{
    dial_peer as conn_dial_peer, AcceptedConn, DialParams, HalfOpenTracker, PeerListener,
    SharedDatagramAdapter,
};
use crate::ratelimit::{Limiter, DEFAULT_DOWNLOAD_BURST};
use crate::sources::{
    PeerSink, RegisterTorrent, Sources, SourcesConfig, TorrentAnnounceInfo, TorrentKeys,
    WebRtcOpenedPeer,
};
use crate::torrent::{AttachInfo, TorrentInner};
use crate::{ClientConfig, CoreError, Result, TcpConnHook, TorrentSpec};
use tokio::sync::Notify;
use wt_wire::handshake::{default_peer_extension_bytes, handshake, handshake_incoming};

/// Thin Sources sink / announce adapter over [`TorrentInner`].
struct TorrentSourcesAdapter {
    torrent: Arc<TorrentInner>,
    peer_id: PeerId,
    listen_port: Arc<RwLock<u16>>,
    peers_low_water: usize,
    always_want_conns: bool,
}

impl PeerSink for TorrentSourcesAdapter {
    fn add_peers(&self, _info_hash: InfoHash, peers: Vec<PeerInfo>) {
        self.torrent.add_peers(peers);
    }

    fn add_web_seeds(&self, _info_hash: InfoHash, urls: Vec<String>) {
        let mut ws = self.torrent.web_seeds.write();
        for u in urls {
            if !ws.contains(&u) {
                ws.push(u);
            }
        }
        self.torrent.bump_change();
    }

    fn add_webrtc_peer(&self, _info_hash: InfoHash, peer: WebRtcOpenedPeer) {
        let Some(client) = self.torrent.client.read().upgrade() else {
            return;
        };
        let torrent = Arc::clone(&self.torrent);
        let handle = client.tokio_handle.clone();
        handle.spawn(async move {
            if let Err(e) = client.attach_webrtc_peer(torrent, peer).await {
                tracing::debug!("webrtc peer attach failed: {e}");
            }
        });
    }
}

/// Synthetic remote addr for a WebRTC data channel (no ICE pair exposed yet).
fn webrtc_synthetic_addr(offer_id: &str, peer_id: PeerId) -> SocketAddr {
    let mut h: u32 = 0x811c_9dc5;
    for b in offer_id.as_bytes().iter().chain(peer_id.iter()) {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    let port = ((h % 55_000) + 10_000) as u16;
    SocketAddr::from(([127, 0, 0, 2], port))
}

/// Lifetime data totals: live and retired connections plus web seeds.
fn transfer_totals(t: &TorrentInner) -> crate::stats::ConnStats {
    let mut cs = t.webseed_stats.snapshot();
    cs.add(&t.retired_conn_stats.lock());
    for h in t.conn_handles.read().values() {
        cs.add(&h.stats().snapshot());
    }
    cs
}

impl TorrentAnnounceInfo for TorrentSourcesAdapter {
    fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Go announces `BytesReadData` (connection totals, not completed pieces).
    fn downloaded(&self) -> i64 {
        transfer_totals(&self.torrent).bytes_read_data
    }

    /// Go `BytesWrittenData`; private trackers compute ratio from it.
    fn uploaded(&self) -> i64 {
        transfer_totals(&self.torrent).bytes_written_data
    }

    fn private(&self) -> bool {
        self.torrent.info.read().as_ref().is_some_and(|i| i.private == Some(true))
    }

    fn left(&self) -> i64 {
        let Some(info) = self.torrent.info.read().clone() else {
            return -1;
        };
        let pieces = self.torrent.pieces.read();
        let mut completed = 0i64;
        for p in pieces.iter() {
            if p.complete {
                completed += info.piece(p.index).length;
            }
        }
        // v2/hybrid pieces hold file data only: pad files are never
        // downloaded, so they are not left either.
        let wanted = if info.has_v2() {
            info.files.iter().filter(|f| !f.is_padding()).map(|f| f.length).sum()
        } else {
            info.total_length()
        };
        (wanted - completed).max(0)
    }

    fn port(&self) -> u16 {
        *self.listen_port.read()
    }

    fn announce_key(&self) -> i32 {
        // Stable-ish per-torrent key from infohash bytes.
        let b = self.torrent.info_hash.0;
        i32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }

    fn need_data(&self) -> bool {
        !self
            .torrent
            .data_download_disallowed
            .load(Ordering::Relaxed)
            && !self.completed()
    }

    fn want_peers(&self) -> bool {
        self.torrent.known_peers.read().len() <= self.peers_low_water
    }

    fn want_conns(&self) -> bool {
        // Go `wantAnyConns` + `AlwaysWantConns`: keep discovering while under
        // established cap (server sets AlwaysWantConns=true).
        if self.torrent.wants_conns() == 0 {
            return false;
        }
        if self.always_want_conns {
            return true;
        }
        self.need_data() || self.torrent.seeding.load(Ordering::Relaxed)
    }

    fn completed(&self) -> bool {
        self.torrent.seeding.load(Ordering::Relaxed)
            || self
                .torrent
                .info
                .read()
                .as_ref()
                .map(|_info| {
                    let pieces = self.torrent.pieces.read();
                    !pieces.is_empty() && pieces.iter().all(|p| p.complete)
                })
                .unwrap_or(false)
    }

    fn newly_completed(&self) -> bool {
        false
    }
}

pub struct ClientInner {
    pub config: ClientConfig,
    pub peer_id: PeerId,
    pub listen_port: Arc<RwLock<u16>>,
    pub listen_addrs: RwLock<Vec<SocketAddr>>,
    pub torrents: RwLock<HashMap<InfoHash, Arc<TorrentInner>>>,
    /// Hybrid magnet fix: both v1 and v2 short hashes (WEBTOR_PATCH).
    pub torrents_by_short: RwLock<HashMap<InfoHash, Arc<TorrentInner>>>,
    /// Serialize lookup, creation, and alias registration for add/merge.
    add_torrent_lock: Mutex<()>,
    pub outgoing_tcp_conn_hook: RwLock<Option<TcpConnHook>>,
    pub urgent_steal_after: RwLock<Duration>,
    /// Router mappings of the listen port, removed on close.
    port_mappings: Mutex<Vec<wt_discovery::portfwd::PortMapping>>,
    /// Redo port mapping and LSD announces now (network changed, torrent added).
    port_map_wake: Arc<tokio::sync::Notify>,
    lsd_wake: Arc<tokio::sync::Notify>,
    pub dial_limiter: Limiter,
    pub download_limiter: Limiter,
    pub upload_limiter: Limiter,
    pub half_open: HalfOpenTracker,
    /// Mirrored for `ClientStats` / older readers; prefer [`HalfOpenTracker::current_total`].
    pub active_half_open: AtomicUsize,
    /// Live dial budget; starts from [`ClientConfig`] and can rise with maxPeers.
    pub half_open_conns_per_torrent: AtomicUsize,
    pub total_half_open_conns: AtomicUsize,
    pub closed: AtomicBool,
    pub tokio_handle: tokio::runtime::Handle,
    _runtime: ClientRuntime,
    listener: RwLock<Option<Arc<PeerListener>>>,
    sources: RwLock<Option<Arc<Sources>>>,
    accept_tx_closed: AtomicBool,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

struct ClientRuntime(Option<tokio::runtime::Runtime>);

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

impl ClientInner {
    pub fn new(cfg: ClientConfig) -> Result<Arc<Self>> {
        let blocking = cfg.piece_hashers_per_torrent.saturating_mul(4).max(16);
        // 4 workers starved dials under Ubuntu stream load (dial_ok ~19 with
        // reader vs ~60 without). Match host CPUs so handshake/dial tasks keep
        // pace with piece IO on the same runtime.
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8)
            .clamp(8, 16);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .max_blocking_threads(blocking)
            .thread_name("wt-client")
            .enable_all()
            .build()?;
        let handle = rt.handle().clone();

        let mut peer_id = [0u8; 20];
        peer_id[..8].copy_from_slice(b"-WT0100-");
        for b in &mut peer_id[8..] {
            *b = rand::random::<u8>();
        }

        let dial_rate = if cfg.dial_rate > 0.0 {
            cfg.dial_rate
        } else {
            100.0
        };
        let dial_burst = dial_rate.max(1.0) as usize;
        let dial_limiter = Limiter::new(dial_rate, dial_burst);

        let download_limiter = match cfg.download_rate {
            Some(r) if r > 0.0 && r.is_finite() => Limiter::new(r, DEFAULT_DOWNLOAD_BURST),
            _ => {
                let l = Limiter::infinite();
                l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
                l
            }
        };
        let upload_limiter = match cfg.upload_rate {
            Some(r) if r > 0.0 && r.is_finite() => Limiter::new(r, DEFAULT_DOWNLOAD_BURST),
            _ => {
                let l = Limiter::infinite();
                l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
                l
            }
        };

        let listen_port = Arc::new(RwLock::new(cfg.listen_port));
        let half_open = HalfOpenTracker::new();
        let half_open_per = cfg.half_open_conns_per_torrent;
        let half_open_total = cfg.total_half_open_conns;

        let client = Arc::new(Self {
            peer_id,
            listen_port: listen_port.clone(),
            listen_addrs: RwLock::new(Vec::new()),
            torrents: RwLock::new(HashMap::new()),
            torrents_by_short: RwLock::new(HashMap::new()),
            add_torrent_lock: Mutex::new(()),
            outgoing_tcp_conn_hook: RwLock::new(None),
            urgent_steal_after: RwLock::new(Duration::ZERO),
            port_mappings: Mutex::new(Vec::new()),
            port_map_wake: Arc::new(tokio::sync::Notify::new()),
            lsd_wake: Arc::new(tokio::sync::Notify::new()),
            dial_limiter,
            download_limiter,
            upload_limiter,
            half_open,
            active_half_open: AtomicUsize::new(0),
            half_open_conns_per_torrent: AtomicUsize::new(half_open_per),
            total_half_open_conns: AtomicUsize::new(half_open_total),
            closed: AtomicBool::new(false),
            tokio_handle: handle.clone(),
            _runtime: ClientRuntime(Some(rt)),
            listener: RwLock::new(None),
            sources: RwLock::new(None),
            accept_tx_closed: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
            config: cfg,
        });

        // Bind listeners + Sources on the client runtime.
        let cl = client.clone();
        let boot = async move { cl.bootstrap_networking().await };
        let boot_res = client.block_on(boot);
        boot_res.map_err(|e| CoreError::Other(format!("client bootstrap: {e}")))?;

        Ok(client)
    }

    async fn bootstrap_networking(self: &Arc<Self>) -> Result<()> {
        let mut utp_for_sources: Option<Arc<wt_wire::utp::UtpSocket>> = None;

        if self.config.accept_peer_connections
            && (!self.config.disable_tcp || !self.config.disable_utp)
        {
            let listener = PeerListener::bind(&self.config).await?;
            let port = listener.listen_port();
            *self.listen_port.write() = port;
            {
                let mut addrs = self.listen_addrs.write();
                addrs.clear();
                addrs.push(SocketAddr::from(([0, 0, 0, 0], port)));
                addrs.push(SocketAddr::from(([127, 0, 0, 1], port)));
                addrs.push(SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)));
                addrs.push(SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)));
            }
            if let Some(utp) = listener.utp_socket() {
                utp_for_sources = Some(Arc::clone(utp));
            }

            let (accepted_tx, mut accepted_rx) = mpsc::channel::<AcceptedConn>(64);
            let weak_lookup_client = Arc::downgrade(self);
            let lookup: crate::conn::InfoHashLookup = Arc::new(move |ih: &InfoHash| {
                weak_lookup_client
                    .upgrade()
                    .is_some_and(|c| c.torrents_by_short.read().contains_key(ih))
            });
            let skey_client = Arc::downgrade(self);
            let skey_provider: crate::conn::SkeyProvider = Arc::new(move || {
                skey_client
                    .upgrade()
                    .map(|c| c.torrents_by_short.read().keys().map(|ih| ih.0).collect())
                    .unwrap_or_default()
            });

            // BEP 52: offer the v2 upgrade on inbound handshakes for torrents
            // with a v2 hash, read live so one learned later counts.
            let v2_client = Arc::downgrade(self);
            listener.set_v2_lookup(Arc::new(move |ih: &InfoHash| {
                v2_client.upgrade().is_some_and(|c| c.inbound_v2_upgrade(ih))
            }));

            listener.spawn_accept_loop(
                self.config.clone(),
                self.peer_id,
                self.half_open.clone(),
                skey_provider,
                lookup,
                accepted_tx,
            );

            let accept_client = Arc::clone(self);
            let task = self.tokio_handle.spawn(async move {
                while let Some(accepted) = accepted_rx.recv().await {
                    if accept_client.closed.load(Ordering::Relaxed) {
                        break;
                    }
                    let cl = accept_client.clone();
                    accept_client.tokio_handle.spawn(async move {
                        let _ = cl.attach_accepted(accepted).await;
                    });
                }
                accept_client
                    .accept_tx_closed
                    .store(true, Ordering::Relaxed);
            });
            self.tasks.lock().push(task);

            *self.listener.write() = Some(Arc::new(listener));
            if self.config.port_forwarding {
                self.start_port_forwarding(port);
            }
            if self.config.local_service_discovery {
                self.start_local_service_discovery(port);
            }
        }

        // Discovery (trackers always; DHT/WebTorrent per config).
        let mut src_cfg = SourcesConfig::from_client_config(&self.config);
        src_cfg.peer_id = self.peer_id;
        src_cfg.dialer = self.config.tracker_dialer.clone();
        src_cfg.listen_port = *self.listen_port.read();
        src_cfg.status_callbacks = self.config.callbacks.status_updated.clone();
        if let Some(utp) = utp_for_sources {
            src_cfg.datagram = Some(Arc::new(SharedDatagramAdapter::new(utp)));
        }
        match Sources::start(src_cfg).await {
            Ok(sources) => {
                *self.sources.write() = Some(sources);
            }
            Err(e) => {
                // Non-fatal when DHT bind fails in constrained test envs.
                tracing::warn!("Sources::start failed: {e}");
            }
        }

        Ok(())
    }

    /// Half-open dial limits for `t`: the live settings, raised during a
    /// peer start burst.
    /// Established connections across all torrents. Locks one at a time.
    pub(crate) fn established_conns(&self) -> usize {
        let torrents: Vec<_> = self.torrents.read().values().cloned().collect();
        torrents.iter().map(|t| t.conn_handles.read().len()).sum()
    }

    pub(crate) fn half_open_limits_for(&self, t: &TorrentInner) -> (usize, usize) {
        let per = self.half_open_conns_per_torrent.load(Ordering::Relaxed);
        let total = self.total_half_open_conns.load(Ordering::Relaxed);
        if t.bursting() {
            let (burst_per, burst_total) = crate::half_open_for_max_peers(crate::torrent::PEER_BURST_CONNS);
            (per.max(burst_per), total.max(burst_total))
        } else {
            (per, total)
        }
    }

    pub(crate) fn rediscover_torrent(&self, t: &TorrentInner) {
        let Some(sources) = self.sources.read().clone() else { return; };
        // The DHT announcer is a Tokio task; callers may be plain threads.
        let _runtime = self.tokio_handle.enter();
        sources.rediscover(&Self::torrent_keys_for(t));
    }

    pub(crate) fn torrent_keys_for(t: &TorrentInner) -> TorrentKeys {
        TorrentKeys {
            v1: Some(t.info_hash),
            v2_short: t.info_hash_v2().map(|v| v.to_short()),
        }
    }

    pub(crate) fn add_trackers(&self, torrent: &TorrentInner, tiers: Vec<Vec<String>>) {
        if tiers.is_empty() {
            return;
        }
        if let Some(sources) = self.sources.read().clone() {
            let _entered = self.tokio_handle.enter();
            let _ = sources.add_trackers(&Self::torrent_keys_for(torrent), tiers);
        }
    }

    fn register_sources(
        &self,
        torrent: &Arc<TorrentInner>,
        keys: TorrentKeys,
        trackers: Vec<Vec<String>>,
        web_seeds: Vec<String>,
    ) {
        let Some(sources) = self.sources.read().clone() else {
            return;
        };
        let adapter = Arc::new(TorrentSourcesAdapter {
            torrent: Arc::clone(torrent),
            peer_id: self.peer_id,
            listen_port: Arc::clone(&self.listen_port),
            peers_low_water: self.config.torrent_peers_low_water,
            always_want_conns: self.config.always_want_conns,
        });
        let reg = RegisterTorrent {
            keys,
            trackers,
            web_seeds,
            info: adapter.clone() as Arc<dyn TorrentAnnounceInfo>,
            sink: adapter as Arc<dyn PeerSink>,
        };
        let _entered = self.tokio_handle.enter();
        // Without its sources a torrent only ever sees peers that find it.
        if let Err(e) = sources.register_torrent(reg) {
            tracing::warn!("register_torrent: {e}");
        }
    }

    /// Add or merge a torrent (Go `Client.AddTorrentSpec` + `Torrent.MergeSpec`).
    pub fn add_torrent_spec(
        self: &Arc<Self>,
        mut spec: TorrentSpec,
    ) -> Result<(Arc<TorrentInner>, bool)> {
        let _add_guard = self.add_torrent_lock.lock();
        if self.closed.load(Ordering::Relaxed) {
            return Err(CoreError::Closed);
        }

        let mut info_obj: Option<wt_types::Info> = None;
        if let Some(ref ib) = spec.info_bytes {
            if let Ok(parsed) = wt_meta::parse_info(ib) {
                // BEP 52: a v2 info dict's swarm key is its SHA-256. Without
                // it a hybrid is only found on the v1 swarm, and a pure v2
                // torrent (no v1 swarm at all) would be keyed by a SHA-1
                // that no peer uses.
                if parsed.has_v2() && spec.info_hash_v2.is_none() {
                    spec.info_hash_v2 = Some(wt_meta::info_hash_v2(ib));
                }
                if spec.info_hash.is_none() && parsed.has_v2() && !parsed.has_v1() {
                    spec.info_hash = spec.info_hash_v2.map(|v2| v2.to_short());
                }
                if spec.info_hash.is_none() {
                    use sha1::{Digest, Sha1};
                    let mut hasher = Sha1::new();
                    hasher.update(ib);
                    let digest: [u8; 20] = hasher.finalize().into();
                    spec.info_hash = Some(InfoHash(digest));
                }
                info_obj = Some(parsed);
            }
        }

        let info_hash = spec
            .info_hash
            .or_else(|| spec.info_hash_v2.map(|v| v.to_short()))
            .ok_or_else(|| CoreError::Other("missing infohash".to_string()))?;

        // Existing by v1 map or short-hash map (hybrid).
        {
            let torrents_map = self.torrents.read();
            let short_map = self.torrents_by_short.read();
            let existing = torrents_map
                .get(&info_hash)
                .cloned()
                .or_else(|| short_map.get(&info_hash).cloned())
                .or_else(|| {
                    spec.info_hash_v2
                        .and_then(|v2| short_map.get(&v2.to_short()).cloned())
                });
            if let Some(existing) = existing {
                if spec
                    .info_hash_v2
                    .and_then(|v2| short_map.get(&v2.to_short()))
                    .is_some_and(|other| !Arc::ptr_eq(other, &existing))
                    || spec
                        .info_hash
                        .and_then(|v1| short_map.get(&v1))
                        .is_some_and(|other| !Arc::ptr_eq(other, &existing))
                {
                    return Err(CoreError::Other(
                        "torrent hashes already belong to different torrents".into(),
                    ));
                }
                drop(short_map);
                drop(torrents_map);
                self.merge_spec(&existing, &spec, info_obj)?;
                if let Some(v1) = spec.info_hash {
                    self.torrents_by_short.write().insert(v1, Arc::clone(&existing));
                    if v1 != existing.info_hash
                        && existing.info_hash_v2().is_none_or(|v2| v2.to_short() != v1)
                    {
                        let trackers = existing.trackers.read().clone();
                        let web_seeds = existing.web_seeds.read().clone();
                        let keys = TorrentKeys { v1: Some(v1), v2_short: None };
                        self.register_sources(&existing, keys, trackers, web_seeds);
                    }
                }
                // A v2 hash learned by merge (e.g. a .torrent or btmh added
                // over a btih magnet) joins the v2 swarm.
                if let Some(v2) = spec.info_hash_v2 {
                    self.adopt_v2_key(&existing, v2);
                }
                return Ok((existing, false));
            }
        }

        let storage = spec
            .storage
            .clone()
            .unwrap_or_else(|| self.config.default_storage.clone());
        let torrent_inner = Arc::new(TorrentInner::new(
            info_hash,
            spec.info_hash_v2,
            spec.display_name.clone(),
            storage,
        ));
        *torrent_inner.client.write() = Arc::downgrade(self);
        torrent_inner
            .max_established_conns
            .store(self.config.established_conns_per_torrent, Ordering::Relaxed);
        if let Some(info) = info_obj {
            torrent_inner.set_info_with_layers(Arc::new(info), &spec.piece_layers)?;
        }
        if let Some(info_bytes) = spec.info_bytes.clone() {
            *torrent_inner.metainfo_raw.write() = Some(wt_types::MetaInfo {
                info_bytes,
                piece_layers: spec.piece_layers.clone(),
                ..Default::default()
            });
        } else if !spec.piece_layers.is_empty() {
            *torrent_inner.metainfo_raw.write() = Some(wt_types::MetaInfo {
                piece_layers: spec.piece_layers.clone(),
                ..Default::default()
            });
        }
        *torrent_inner.trackers.write() = spec.trackers.clone();
        *torrent_inner.web_seeds.write() = spec.web_seeds.clone();

        {
            let mut torrents_map = self.torrents.write();
            let mut short_map = self.torrents_by_short.write();
            torrents_map.insert(info_hash, torrent_inner.clone());
            short_map.insert(info_hash, torrent_inner.clone());
            if let Some(v2) = spec.info_hash_v2 {
                short_map.insert(v2.to_short(), torrent_inner.clone());
            }
        }

        if !spec.dht_nodes.is_empty() {
            self.add_dht_nodes(&spec.dht_nodes);
        }

        self.register_sources(
            &torrent_inner,
            Self::torrent_keys_for(&torrent_inner),
            spec.trackers.clone(),
            spec.web_seeds.clone(),
        );

        // A magnet's first dials already burst: peers turned away at the
        // configured limit are backed off before its download is allowed.
        // Torrents with info burst when their download is allowed.
        torrent_inner.start_peer_burst();
        if self.config.dial_for_peer_conns {
            self.start_dialer(&torrent_inner);
        }
        self.lsd_wake.notify_one();
        let task = self.tokio_handle.spawn(crate::webseed_download::run(
            Arc::downgrade(self),
            Arc::downgrade(&torrent_inner),
        ));
        let mut tasks = self.tasks.lock();
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
        drop(tasks);

        Ok((torrent_inner, true))
    }

    fn merge_spec(
        &self,
        torrent: &Arc<TorrentInner>,
        spec: &TorrentSpec,
        info_obj: Option<wt_types::Info>,
    ) -> Result<()> {
        // display_name is not interior-mutable on TorrentInner; set only at create.
        if let Some(info) = info_obj {
            if torrent.info.read().is_none() {
                torrent.set_info_with_layers(Arc::new(info), &spec.piece_layers)?;
            } else if !spec.piece_layers.is_empty() {
                torrent.apply_piece_layers(&spec.piece_layers)?;
            }
        } else if !spec.piece_layers.is_empty() {
            torrent.apply_piece_layers(&spec.piece_layers)?;
        }
        if let Some(info_bytes) = &spec.info_bytes {
            let mut mi = wt_types::MetaInfo {
                info_bytes: info_bytes.clone(),
                ..Default::default()
            };
            if !spec.piece_layers.is_empty() {
                mi.piece_layers = spec.piece_layers.clone();
            } else if let Some(prev) = torrent.metainfo_raw.read().as_ref() {
                mi.piece_layers = prev.piece_layers.clone();
            }
            *torrent.metainfo_raw.write() = Some(mi);
        } else if !spec.piece_layers.is_empty() {
            if let Some(raw) = torrent.metainfo_raw.write().as_mut() {
                raw.piece_layers = spec.piece_layers.clone();
            }
        }
        if !spec.dht_nodes.is_empty() {
            self.add_dht_nodes(&spec.dht_nodes);
        }
        {
            let mut ws = torrent.web_seeds.write();
            for u in &spec.web_seeds {
                if !ws.contains(u) {
                    ws.push(u.clone());
                }
            }
        }
        {
            let mut tr = torrent.trackers.write();
            for tier in &spec.trackers {
                if !tr.iter().any(|t| t == tier) {
                    tr.push(tier.clone());
                }
            }
        }

        if let Some(sources) = self.sources.read().clone() {
            let _entered = self.tokio_handle.enter();
            let keys = Self::torrent_keys_for(torrent);
            let _ = sources.add_trackers(&keys, spec.trackers.clone());
            if !spec.web_seeds.is_empty() {
                // Sink path also used at register; merge into torrent above is enough for now.
            }
            sources.refresh_torrent(&keys);
        }

        torrent.bump_change();
        Ok(())
    }

    /// BEP 52: give `torrent` its v2 infohash after creation and join the
    /// v2 swarm (short-hash alias for inbound handshakes, trackers, DHT;
    /// LSD and dials read it live). `false` when it already has another v2
    /// hash or the short hash belongs to another torrent.
    pub(crate) fn adopt_v2_key(&self, torrent: &Arc<TorrentInner>, v2: InfoHashV2) -> bool {
        if torrent.info_hash_v2() == Some(v2) {
            return true;
        }
        let short = v2.to_short();
        {
            let mut aliases = self.torrents_by_short.write();
            if aliases.get(&short).is_some_and(|other| !Arc::ptr_eq(other, torrent)) {
                return false;
            }
            if !torrent.set_info_hash_v2(v2) {
                return false;
            }
            aliases.insert(short, Arc::clone(torrent));
        }
        if short != torrent.info_hash && !torrent.closed.load(Ordering::Relaxed) {
            let trackers = torrent.trackers.read().clone();
            let web_seeds = torrent.web_seeds.read().clone();
            let keys = TorrentKeys { v1: None, v2_short: Some(short) };
            self.register_sources(torrent, keys, trackers, web_seeds);
        }
        wt_types::events::record("v2_key_adopted", short.to_hex());
        true
    }

    /// Whether an inbound handshake for `ih` should carry the BEP 52 v2
    /// upgrade bit: the torrent it names has a v2 infohash.
    pub(crate) fn inbound_v2_upgrade(&self, ih: &InfoHash) -> bool {
        self.torrents_by_short.read().get(ih).is_some_and(|t| {
            t.info_hash_v2().is_some() || t.info.read().as_ref().is_some_and(|i| i.has_v2())
        })
    }

    pub fn torrent(&self, ih: InfoHash) -> Option<Arc<TorrentInner>> {
        self.torrents
            .read()
            .get(&ih)
            .cloned()
            .or_else(|| self.torrents_by_short.read().get(&ih).cloned())
    }

    pub fn torrents(&self) -> Vec<Arc<TorrentInner>> {
        self.torrents.read().values().cloned().collect()
    }

    pub(crate) fn detach_torrent(&self, torrent: &Arc<TorrentInner>) {
        let _add_guard = self.add_torrent_lock.lock();
        let hashes: Vec<InfoHash> = self.torrents_by_short.read().iter()
            .filter(|(_, entry)| Arc::ptr_eq(entry, torrent))
            .map(|(hash, _)| *hash)
            .collect();
        self.torrents.write().retain(|_, entry| !Arc::ptr_eq(entry, torrent));
        self.torrents_by_short.write().retain(|_, entry| !Arc::ptr_eq(entry, torrent));
        // Stop its trackers and DHT. Left registered, they kept announcing for
        // the dead torrent and made a re-add of the hash start without discovery.
        if let Some(sources) = self.sources.read().clone() {
            let _entered = self.tokio_handle.enter();
            for hash in hashes {
                sources.unregister_torrent(&TorrentKeys { v1: Some(hash), v2_short: None });
            }
        }
    }

    pub fn local_port(&self) -> u16 {
        *self.listen_port.read()
    }

    pub fn listen_addrs(&self) -> Vec<SocketAddr> {
        self.listen_addrs.read().clone()
    }

    pub fn add_dht_nodes(&self, nodes: &[String]) {
        if let Some(sources) = self.sources.read().clone() {
            let _entered = self.tokio_handle.enter();
            sources.add_dht_nodes(nodes);
        }
    }

    pub fn dht_contacts(&self) -> Vec<SocketAddr> {
        let Some(sources) = self.sources.read().clone() else {
            return Vec::new();
        };
        self.block_on(async move { sources.dht_contacts().await })
    }

    fn block_on<F: std::future::Future + Send>(&self, fut: F) -> F::Output
    where
        F::Output: Send,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(|| self.tokio_handle.block_on(fut))
                    .join()
                    .expect("client runtime operation panicked")
            })
        } else {
            self.tokio_handle.block_on(fut)
        }
    }

    pub fn set_outgoing_tcp_conn_hook(&self, hook: Option<TcpConnHook>) {
        *self.outgoing_tcp_conn_hook.write() = hook;
    }

    pub fn set_urgent_request_stealing(&self, after: Duration) {
        *self.urgent_steal_after.write() = after;
    }

    /// The network changed: start trackers on fresh connection state, drop what
    /// was learned about handshakes on the old network, map the port on the new
    /// router and announce to the new local network.
    pub fn network_changed(&self) {
        crate::conn::forget_first_try_stats();
        if let Some(sources) = self.sources.read().clone() {
            sources.reset_tracker_clients();
        }
        self.port_map_wake.notify_one();
        self.lsd_wake.notify_one();
    }

    /// Go `forwardPort`, renewed every 30 minutes (leases, router restarts)
    /// and after network changes.
    fn start_port_forwarding(self: &Arc<Self>, port: u16) {
        let weak = Arc::downgrade(self);
        let wake = Arc::clone(&self.port_map_wake);
        let description = self.config.http_user_agent.clone();
        let task = self.tokio_handle.spawn(async move {
            loop {
                let mappings = wt_discovery::portfwd::map_port(port, &description).await;
                match weak.upgrade() {
                    Some(client) if !client.closed.load(Ordering::Relaxed) => {
                        *client.port_mappings.lock() = mappings;
                    }
                    _ => {
                        wt_discovery::portfwd::unmap(&mappings).await;
                        return;
                    }
                }
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30 * 60)) => {}
                }
            }
        });
        self.tasks.lock().push(task);
    }

    /// BEP 14: announce public torrents to the local network every five
    /// minutes (sooner when one is added) and add the peers others announce.
    fn start_local_service_discovery(self: &Arc<Self>, port: u16) {
        use wt_discovery::lsd;
        let weak = Arc::downgrade(self);
        let wake = Arc::clone(&self.lsd_wake);
        let cookie = hex::encode(rand::random::<[u8; 8]>());
        let task = self.tokio_handle.spawn(async move {
            let Ok(socket) = lsd::bind() else { return };
            let group = SocketAddr::from((lsd::LSD_GROUP, lsd::LSD_PORT));
            let mut buf = [0u8; 1500];
            let mut last: Option<tokio::time::Instant> = None;
            let mut next = tokio::time::Instant::now();
            loop {
                if tokio::time::Instant::now() >= next {
                    let Some(client) = weak.upgrade() else { return };
                    if client.closed.load(Ordering::Relaxed) {
                        return;
                    }
                    let hashes = client.lsd_info_hashes();
                    drop(client);
                    for chunk in hashes.chunks(lsd::HASHES_PER_ANNOUNCE) {
                        let _ = socket.send_to(lsd::announce(port, &cookie, chunk).as_bytes(), group).await;
                    }
                    last = Some(tokio::time::Instant::now());
                    next = tokio::time::Instant::now() + lsd::ANNOUNCE_EVERY;
                }
                tokio::select! {
                    received = socket.recv_from(&mut buf) => {
                        let Ok((n, from)) = received else { continue };
                        let Some(announce) = lsd::parse(&buf[..n]) else { continue };
                        if announce.cookie.as_deref() == Some(cookie.as_str()) {
                            continue;
                        }
                        let Some(client) = weak.upgrade() else { return };
                        client.add_lsd_peers(&announce, SocketAddr::new(from.ip(), announce.port));
                    }
                    // BEP 14: at most one announce a minute.
                    _ = wake.notified() => {
                        next = last.map_or(tokio::time::Instant::now(), |at| at + Duration::from_secs(60));
                    }
                    _ = tokio::time::sleep_until(next) => {}
                }
            }
        });
        self.tasks.lock().push(task);
    }

    /// Public torrents with metadata. BEP 27: private torrents use no LSD.
    /// A hybrid is announced under both swarm keys (v1 and truncated v2), as
    /// for trackers and the DHT, so v2-only LAN peers find it too.
    pub(crate) fn lsd_info_hashes(&self) -> Vec<String> {
        let torrents: Vec<_> = self.torrents.read().values().cloned().collect();
        torrents.iter()
            .filter(|t| !t.closed.load(Ordering::Relaxed)
                && t.info.read().as_ref().is_some_and(|i| i.private != Some(true)))
            .flat_map(|t| Self::torrent_keys_for(t).all_short())
            .map(|ih| hex::encode(ih.0))
            .collect()
    }

    fn add_lsd_peers(&self, announce: &wt_discovery::lsd::Announce, addr: SocketAddr) {
        for hash in &announce.info_hashes {
            let Some(t) = self.torrents_by_short.read().get(&InfoHash(*hash)).cloned() else { continue };
            if t.closed.load(Ordering::Relaxed)
                || !t.info.read().as_ref().is_some_and(|i| i.private != Some(true))
            {
                continue;
            }
            t.add_peers(vec![PeerInfo {
                id: None,
                addr,
                source: PeerSource::Lsd,
                supports_encryption: false,
                pex: Default::default(),
                trusted: false,
            }]);
        }
    }

    pub fn close(self: &Arc<Self>) {
        let _add_guard = self.add_torrent_lock.lock();
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }

        if let Some(listener) = self.listener.write().take() {
            listener.close();
        }

        for task in self.tasks.lock().drain(..) {
            task.abort();
        }
        // Go `clearPortMappings`; a router that stopped answering must not
        // hold up shutdown.
        let mappings = std::mem::take(&mut *self.port_mappings.lock());
        if !mappings.is_empty() {
            self.block_on(async move {
                let _ = tokio::time::timeout(Duration::from_millis(1500),
                    wt_discovery::portfwd::unmap(&mappings)).await;
            });
        }

        if let Some(sources) = self.sources.write().take() {
            self.block_on(async move { sources.close().await });
        }

        let torrents: Vec<_> = {
            let mut map = self.torrents.write();
            let vals: Vec<_> = map.values().cloned().collect();
            map.clear();
            vals
        };
        self.torrents_by_short.write().clear();

        for t in torrents {
            t.closed.store(true, Ordering::Relaxed);
            for c in t.conn_handles.write().drain().map(|(_, c)| c) {
                c.close("client closed");
            }
        }
    }

    fn start_dialer(self: &Arc<Self>, torrent: &Arc<TorrentInner>) {
        let client = Arc::downgrade(self);
        let torrent = Arc::downgrade(torrent);
        let task = self.tokio_handle.spawn(async move {
            let mut last_announce_refresh = Instant::now() - Duration::from_secs(60);
            loop {
                let (Some(client), Some(torrent)) = (client.upgrade(), torrent.upgrade()) else {
                    break;
                };
                if client.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
                    break;
                }
                torrent.end_peer_burst_if_due();
                if torrent.bursting() {
                    torrent.update_peer_burst_limit(client.established_conns());
                }
                let count = if torrent.wants_conns() > 0 {
                    let (per, total) = client.half_open_limits_for(&torrent);
                    torrent.max_half_open(per)
                        .saturating_sub(torrent.dialing_peers.lock().len())
                        .min(total.saturating_sub(client.half_open.current_total()))
                } else { 0 };
                // Refresh want_peers snapshot every 5s while under low-water so
                // tracker slots can be nudged (see AnnounceDispatcher::update_torrent).
                if torrent.known_peers.read().len() <= client.config.torrent_peers_low_water
                    && last_announce_refresh.elapsed() >= Duration::from_secs(5)
                {
                    last_announce_refresh = Instant::now();
                    if let Some(sources) = client.sources.read().clone() {
                        sources.refresh_torrent(&ClientInner::torrent_keys_for(&torrent));
                    }
                }
                for peer in torrent.take_dial_candidates(count) {
                    if peer.source == PeerSource::Pex {
                        torrent.swarm_diagnostics.lock().pex_dialed += 1;
                    }
                    let cl = Arc::clone(&client);
                    let tor = Arc::clone(&torrent);
                    let task = client.tokio_handle.spawn(async move {
                        let mut route = tor.route_epoch.subscribe();
                        let result = tokio::select! {
                            result = cl.dial_peer(Arc::clone(&tor), peer.addr, peer.source) => result,
                            _ = route.changed() => {
                                // Not this peer's fault: back in the pool, no penalty.
                                tor.add_peers(vec![peer.clone()]);
                                tor.dial_finished(peer.addr, true);
                                return;
                            }
                        };
                        let ok = result.is_ok();
                        // Failures that say nothing bad about the peer (no
                        // half-open slot, no room, already connected) must
                        // not cost it a fail count: see `DialVerdict`.
                        let verdict = crate::torrent::dial_verdict(&result);
                        if !matches!(
                            verdict,
                            crate::torrent::DialVerdict::Connected | crate::torrent::DialVerdict::Unreachable
                        ) {
                            if verdict.reached_peer() {
                                tor.note_dial_source(peer.source, true);
                                wt_types::events::record("dial_skip", format!(
                                    "src={:?} kind={verdict:?}", peer.source));
                            }
                            tor.dial_settled(&peer, verdict);
                            return;
                        }
                        if let Err(ref e) = result {
                            let msg = e.to_string();
                            *tor.swarm_diagnostics.lock().dial_failures
                                .entry(crate::stats::failure_kind(&msg).into()).or_default() += 1;
                            // Dial-failure taxonomy for the desktop peers_compare
                            // tool (stderr → engine-host log). Android discards
                            // stderr, so it is not built there.
                            // Align with Go webtorFailureKind: race failures are
                            // "other" ("all initial dials failed"); timeouts in
                            // that message must not dominate dialFailures.
                            #[cfg(not(target_os = "android"))]
                            {
                            let kind = if msg.contains("all initial dials failed") {
                                "other"
                            } else if msg.contains("MSE") {
                                "mse"
                            } else if msg.contains("BitTorrent handshake")
                                || msg.contains("handshake failed")
                            {
                                "handshake"
                            } else if msg.contains("timeout") || msg.contains("timed out") {
                                "timeout"
                            } else {
                                "other"
                            };
                            eprintln!(
                                "dial_fail kind={kind} src={:?} addr={} err={}",
                                peer.source,
                                peer.addr,
                                msg.chars().take(160).collect::<String>()
                            );
                            }
                        }
                        // Go PopMax: failed dials leave the pool; rediscovery
                        // (tracker/DHT/PEX mid-run feed) re-adds them. Requeue
                        // kept pending≈200 → reducedDialTimeout stuck at min
                        // (Bunny retain3: ~565 timeouts/60s) while Go's shrinking
                        // peers.Len() allowed longer dials and a higher mid-run
                        // peer spike. Keep fail_count/backoff so rediscovery
                        // cannot immediately redial the same dead contact.
                        tor.note_dial_source(peer.source, ok);
                        if let Err(ref e) = result {
                            wt_types::events::record("dial_fail", format!(
                                "src={:?} kind={}", peer.source, crate::stats::failure_kind(&e.to_string())));
                        } else {
                            wt_types::events::record("dial_ok", format!("src={:?}", peer.source));
                        }
                        if !ok {
                            tor.dial_finished(peer.addr, false);
                            // BEP 55: a peer that can be punched may be behind
                            // a NAT; ask a relay to introduce us.
                            tor.maybe_rendezvous(&peer);
                        } else {
                            tor.swarm_diagnostics.lock().dial_ok += 1;
                            #[cfg(not(target_os = "android"))]
                            {
                            let transport = tor
                                .conn_handles
                                .read()
                                .get(&peer.addr)
                                .map(|h| format!("{:?}", h.transport()))
                                .unwrap_or_else(|| "?".into());
                            eprintln!(
                                "dial_ok src={:?} transport={} addr={}",
                                peer.source, transport, peer.addr
                            );
                            }
                            tor.dial_finished(peer.addr, true);
                        }
                    });
                    let mut tasks = client.tasks.lock();
                    tasks.retain(|t| !t.is_finished());
                    tasks.push(task);
                }
                // Poll half-open turnover faster than 1s (Go openNewConns is event-driven),
                // but only while dialing. Otherwise new peers wake this anyway.
                let dialing = torrent.bursting() || !torrent.dialing_peers.lock().is_empty();
                tokio::select! {
                    _ = torrent.peers_changed().notified() => {},
                    _ = tokio::time::sleep(Duration::from_millis(if dialing { 100 } else { 1000 })) => {},
                }
            }
        });
        self.tasks.lock().push(task);
    }

    async fn attach_accepted(self: &Arc<Self>, accepted: AcceptedConn) -> Result<()> {
        let torrent = self
            .torrents_by_short
            .read()
            .get(&accepted.info_hash)
            .cloned()
            .ok_or_else(|| CoreError::Other("unknown infohash after accept".into()))?;
        // BEP 55: the remote's simultaneous uTP dial can be the one that
        // gets through, arriving here as an inbound connection.
        let remote = accepted.handle.remote_addr();
        let punched = accepted.handle.transport() == wt_types::Transport::Utp
            && torrent.holepunch.lock().is_punched(remote, Instant::now());
        let (source, handle) = if punched {
            (PeerSource::UtHolepunch, accepted.handle.with_source(PeerSource::UtHolepunch))
        } else {
            (PeerSource::Incoming, accepted.handle)
        };
        let info = AttachInfo {
            remote,
            outgoing: false,
            transport: handle.transport(),
            encrypted: handle.is_encrypted(),
            peer_id: accepted.peer_id,
            reserved: accepted.peer_extension_bits.0,
            source,
        };
        self.attach_driver(
            torrent,
            info,
            accepted.stream,
            handle,
            accepted.write_rx,
            accepted.write_waker,
        )
    }

    /// Go `Torrent.onWebRtcConn`: BT handshake over the data channel (no MSE).
    async fn attach_webrtc_peer(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        peer: WebRtcOpenedPeer,
    ) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
            return Err(CoreError::Closed);
        }
        if peer.info_hash != torrent.info_hash
            && !self
                .torrents_by_short
                .read()
                .get(&peer.info_hash)
                .is_some_and(|t| Arc::ptr_eq(t, &torrent))
        {
            return Err(CoreError::Other("webrtc infohash mismatch".into()));
        }

        let remote = webrtc_synthetic_addr(&peer.offer_id, peer.peer_id);
        if torrent.conn_handles.read().contains_key(&remote) {
            return Ok(());
        }

        let outgoing = peer.local_offered;
        let source = if outgoing {
            PeerSource::Tracker
        } else {
            PeerSource::Incoming
        };
        let mut stream: Box<dyn crate::PeerStream> = Box::new(peer.stream);
        let mut reserved = default_peer_extension_bytes();
        let hs_timeout = self.config.handshakes_timeout.max(Duration::from_secs(5));
        let v2_short = torrent.info_hash_v2().map(|v| v.to_short());
        if v2_short.is_some_and(|s| s != torrent.info_hash) {
            reserved.set_bit(wt_wire::handshake::EXTENSION_BIT_V2_UPGRADE, true);
        }
        let peer_hs = if outgoing {
            let fut = handshake(
                &mut stream,
                &torrent.info_hash,
                &self.peer_id,
                &reserved,
            );
            tokio::time::timeout(hs_timeout, fut)
                .await
                .map_err(|_| CoreError::Other("webrtc handshake timeout".into()))?
                .map_err(|e| CoreError::Other(format!("webrtc handshake: {e}")))?
        } else {
            let expected = torrent.info_hash;
            let fut = handshake_incoming(&mut stream, &self.peer_id, &reserved, |ih| {
                *ih == expected
                    || self.torrents_by_short.read().contains_key(ih)
            });
            tokio::time::timeout(hs_timeout, fut)
                .await
                .map_err(|_| CoreError::Other("webrtc incoming handshake timeout".into()))?
                .map_err(|e| CoreError::Other(format!("webrtc incoming handshake: {e}")))?
        };

        let negotiated_ih = if peer_hs.info_hash == torrent.info_hash {
            torrent.info_hash
        } else if v2_short.is_some_and(|s| peer_hs.info_hash == s) {
            peer_hs.info_hash
        } else if self.torrents_by_short.read().contains_key(&peer_hs.info_hash) {
            peer_hs.info_hash
        } else {
            torrent.info_hash
        };

        let (write_tx, write_rx) = mpsc::unbounded_channel();
        let write_waker = Arc::new(Notify::new());
        let closed = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(crate::stats::AtomicConnStats::default());
        let handle = crate::conn::ConnHandle::new(
            remote,
            outgoing,
            wt_types::Transport::WebRtc,
            false,
            negotiated_ih,
            Some(peer_hs.peer_id),
            source,
            peer_hs.extensions.0,
            write_tx,
            write_waker.clone(),
            closed,
            stats,
        );
        let info = AttachInfo {
            remote,
            outgoing,
            transport: wt_types::Transport::WebRtc,
            encrypted: false,
            peer_id: peer_hs.peer_id,
            reserved: peer_hs.extensions.0,
            source,
        };
        self.attach_driver(torrent, info, stream, handle, write_rx, write_waker)
    }

    pub async fn dial_peer(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        addr: SocketAddr,
        source: PeerSource,
    ) -> Result<()> {
        // None => dialer races TCP+uTP under one half-open slot (Go dialPool).
        self.dial_peer_transport(torrent, addr, source, None, false)
            .await
    }

    pub async fn dial_peer_transport(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        addr: SocketAddr,
        source: PeerSource,
        transport: Option<wt_types::Transport>,
        require_plain: bool,
    ) -> Result<()> {
        self.dial_peer_inner(torrent, addr, source, transport, require_plain, None)
            .await
    }

    /// The uTP listen socket a dial to `addr` leaves from (IPv4 or IPv6).
    pub(crate) fn utp_socket_for(&self, addr: SocketAddr) -> Option<Arc<wt_wire::utp::UtpSocket>> {
        self.listener
            .read()
            .as_ref()
            .and_then(|l| l.utp_socket_for(addr).cloned())
    }

    /// Whether a BEP 55 punch toward `endpoint` is possible at all: uTP is
    /// on, we listen on it (punches must leave from the listen port), and
    /// the listen socket's address family matches.
    pub(crate) fn can_punch(&self, endpoint: SocketAddr) -> bool {
        if self.closed.load(Ordering::Relaxed)
            || self.config.disable_utp
            || self.config.disable_holepunch
            || !self.config.dial_for_peer_conns
            || !crate::holepunch::usable_endpoint(endpoint)
        {
            return false;
        }
        let Some(sock) = self.utp_socket_for(endpoint) else {
            return false;
        };
        match sock.local_addr() {
            Ok(local) => {
                local.is_ipv4() == endpoint.is_ipv4()
                    && !(endpoint.ip().is_loopback() && endpoint.port() == local.port())
            }
            Err(_) => false,
        }
    }

    /// BEP 55: dial `endpoint` over uTP from the listen socket, resending the
    /// SYN so it survives the first packets dying at either NAT. Goes through
    /// the normal dial path: half-open limits, dial rate limiter (Ghost
    /// Mode), encryption policy and the connection limit all apply.
    pub(crate) fn spawn_punch_dial(self: &Arc<Self>, torrent: Arc<TorrentInner>, endpoint: SocketAddr) {
        if !self.can_punch(endpoint) || torrent.has_peer_at(endpoint) {
            return;
        }
        let Some(utp) = self.utp_socket_for(endpoint) else { return };
        // The uTP socket is shared by all torrents: a stream to this endpoint
        // that another torrent holds says nothing about this one's punch.
        if utp.has_stream_to(endpoint) && !self.endpoint_held_by_other_torrent(&torrent, endpoint) {
            // The remote's SYN already got through; that connection is it.
            torrent.holepunch.lock().note_punch(endpoint, Instant::now());
            return;
        }
        if !torrent.dialing_peers.lock().insert(endpoint) {
            return;
        }
        torrent.holepunch.lock().note_punch(endpoint, Instant::now());
        crate::holepunch::HolepunchCounters::bump(&torrent.holepunch_stats.punch_dials_attempted);
        let cl = Arc::clone(self);
        let task = self.tokio_handle.spawn(async move {
            let result = cl
                .dial_peer_inner(
                    Arc::clone(&torrent),
                    endpoint,
                    PeerSource::UtHolepunch,
                    Some(wt_types::Transport::Utp),
                    false,
                    Some(crate::holepunch::PUNCH_SYN_RESEND),
                )
                .await;
            torrent.dialing_peers.lock().remove(&endpoint);
            match &result {
                Ok(()) => wt_types::events::record("holepunch_dial_ok", format!("addr={endpoint}")),
                Err(e) => wt_types::events::record(
                    "holepunch_dial_fail",
                    format!("kind={}", crate::stats::failure_kind(&e.to_string())),
                ),
            }
            // No dial penalty: a failed punch says nothing about the peer.
            torrent.peers_changed().notify_one();
        });
        let mut tasks = self.tasks.lock();
        tasks.retain(|t| !t.is_finished());
        tasks.push(task);
    }

    /// Another torrent is connected or dialing to `endpoint`.
    pub(crate) fn endpoint_held_by_other_torrent(
        &self,
        torrent: &Arc<TorrentInner>,
        endpoint: SocketAddr,
    ) -> bool {
        self.torrents()
            .iter()
            .any(|t| !Arc::ptr_eq(t, torrent) && t.has_peer_at(endpoint))
    }

    async fn dial_peer_inner(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        addr: SocketAddr,
        source: PeerSource,
        transport: Option<wt_types::Transport>,
        require_plain: bool,
        utp_syn_resend: Option<Duration>,
    ) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
            return Err(CoreError::Closed);
        }
        if torrent.conn_handles.read().contains_key(&addr) {
            return Ok(());
        }
        let metadata_size = torrent
            .metainfo_raw
            .read()
            .as_ref()
            .map(|i| i.info_bytes.len() as u32);
        let hook = self.outgoing_tcp_conn_hook.read().clone();
        let mut cfg = self.config.clone();
        // Live limits: Settings changes and peer bursts apply without restart
        // (the startup config kept the dialer at the launch-time limit).
        let (per_torrent, total) = self.half_open_limits_for(&torrent);
        cfg.half_open_conns_per_torrent = per_torrent;
        cfg.total_half_open_conns = total;
        // Diagnostic: WT_FORCE_UTP=1 dials uTP only (Ubuntu Go holds ~25% uTP).
        if std::env::var_os("WT_FORCE_UTP").is_some() {
            cfg.disable_tcp = true;
        }
        // Go reducedDialTimeout(min, nominal, HalfOpenConnsPerTorrent, peers.Len()).
        // Count only ready (non-backoff) reserve peers so a pool of cooling-down
        // failures does not clamp every retry to MinDialTimeout.
        let pending = torrent.dial_pending_ready();
        cfg.nominal_dial_timeout = crate::conn::reduced_dial_timeout(
            cfg.min_dial_timeout,
            cfg.nominal_dial_timeout,
            per_torrent,
            pending,
        );
        // A contact that already failed gets a shorter dial: it holds a
        // half-open slot for less time. Never a BEP 55 punch, whose target
        // failed a direct dial by definition.
        if utp_syn_resend.is_none() {
            let fails = torrent.dial_fail_counts.lock().get(&addr).copied().unwrap_or(0);
            cfg.nominal_dial_timeout = crate::torrent::retry_dial_timeout(
                cfg.nominal_dial_timeout,
                cfg.min_dial_timeout,
                fails,
            );
        }
        let utp = self.utp_socket_for(addr);
        let outcome = conn_dial_peer(DialParams {
            remote_addr: addr,
            info_hash: torrent.info_hash,
            info_hash_v2_short: torrent.info_hash_v2().map(|v| v.to_short()),
            our_peer_id: self.peer_id,
            transport,
            source,
            metadata_size,
            cfg: &cfg,
            tracker: &self.half_open,
            dial_limiter: Some(&self.dial_limiter),
            tcp_conn_hook: hook.as_ref(),
            utp_socket: utp.as_ref(),
            require_plain,
            utp_syn_resend,
        })
        .await?;
        let info = AttachInfo {
            remote: addr,
            outgoing: true,
            transport: outcome.handle.transport(),
            encrypted: outcome.handle.is_encrypted(),
            peer_id: outcome.peer_id,
            reserved: outcome.peer_extension_bits.0,
            source,
        };
        match self.attach_driver(
            torrent.clone(),
            info,
            outcome.stream,
            outcome.handle,
            outcome.write_rx,
            outcome.write_waker,
        ) {
            Ok(()) => Ok(()),
            Err(e) => {
                if torrent.conn_handles.read().contains_key(&addr) {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn attach_driver(
        self: &Arc<Self>,
        torrent: Arc<TorrentInner>,
        info: AttachInfo,
        stream: Box<dyn crate::PeerStream>,
        handle: crate::conn::ConnHandle,
        write_rx: mpsc::UnboundedReceiver<crate::wire::Message>,
        write_waker: Arc<tokio::sync::Notify>,
    ) -> Result<()> {
        if self.closed.load(Ordering::Relaxed) || torrent.closed.load(Ordering::Relaxed) {
            return Err(CoreError::Closed);
        }
        let handler = torrent.attach_connection(info, handle.clone())?;
        for cb in &self.config.callbacks.completed_handshake {
            cb(&handle.info(), torrent.info_hash);
        }
        let (reader, writer) = crate::conn::spawn_connection_driver(
            stream,
            handler,
            handle,
            write_rx,
            write_waker,
            Some(self.download_limiter.clone()),
            Some(self.upload_limiter.clone()),
            crate::conn::DEFAULT_KEEPALIVE_TIMEOUT,
            crate::conn::DEFAULT_READ_DEADLINE,
        );
        let mut tasks = self.tasks.lock();
        tasks.retain(|t| !t.is_finished());
        tasks.push(reader);
        tasks.push(writer);
        Ok(())
    }
}

#[cfg(test)]
mod bt20_tests {
    use super::*;

    const PURE_V2: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-test.torrent");
    const HYBRID: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");

    fn client() -> crate::Client {
        let cfg = ClientConfig {
            no_dht: true,
            local_service_discovery: false,
            ..Default::default()
        };
        crate::Client::new(cfg).unwrap()
    }

    fn add_info_only(c: &crate::Client, file: &[u8]) -> Arc<TorrentInner> {
        let mi = wt_meta::parse_metainfo(file).unwrap();
        let spec = TorrentSpec {
            info_bytes: Some(mi.info_bytes),
            ..Default::default()
        };
        c.inner.add_torrent_spec(spec).unwrap().0
    }

    /// A pure v2 info dict added without hashes is keyed by its truncated
    /// SHA-256 (the only swarm it has), not by a SHA-1 of the info bytes.
    #[test]
    fn pure_v2_info_bytes_key_the_torrent_by_truncated_sha256() {
        let c = client();
        let t = add_info_only(&c, PURE_V2);
        let ib = wt_meta::parse_metainfo(PURE_V2).unwrap().info_bytes;
        let v2 = wt_meta::info_hash_v2(&ib);
        assert_eq!(t.info_hash_v2(), Some(v2));
        assert_eq!(t.info_hash, v2.to_short());
        assert_ne!(t.info_hash, wt_meta::info_hash_v1(&ib));
        assert!(c.inner.torrents_by_short.read().contains_key(&v2.to_short()));
        c.close();
    }

    /// A hybrid info dict joins both swarms, and LSD announces both keys.
    #[test]
    fn hybrid_info_bytes_register_both_swarms_and_lsd_announces_both() {
        let c = client();
        let t = add_info_only(&c, HYBRID);
        let ib = wt_meta::parse_metainfo(HYBRID).unwrap().info_bytes;
        let (v1, v2) = (wt_meta::info_hash_v1(&ib), wt_meta::info_hash_v2(&ib));
        assert_eq!(t.info_hash, v1);
        assert_eq!(t.info_hash_v2(), Some(v2));
        let short = c.inner.torrents_by_short.read();
        assert!(short.contains_key(&v1) && short.contains_key(&v2.to_short()));
        drop(short);
        let mut lsd = c.inner.lsd_info_hashes();
        lsd.sort();
        let mut want = vec![v1.to_hex(), v2.to_short().to_hex()];
        want.sort();
        assert_eq!(lsd, want);
        c.close();
    }

    /// BEP 55: a uTP stream to an endpoint held by another torrent must not
    /// pass for this torrent's punch having got through.
    #[test]
    fn endpoint_held_only_counts_other_torrents() {
        let c = client();
        let a = add_info_only(&c, PURE_V2);
        let b = add_info_only(&c, HYBRID);
        let ep: SocketAddr = "203.0.113.7:6881".parse().unwrap();
        assert!(!c.inner.endpoint_held_by_other_torrent(&a, ep));
        b.dialing_peers.lock().insert(ep);
        assert!(c.inner.endpoint_held_by_other_torrent(&a, ep));
        assert!(!c.inner.endpoint_held_by_other_torrent(&b, ep), "its own dial is not another's");
        c.close();
    }

    /// Pure v2: one key, announced once.
    #[test]
    fn pure_v2_lsd_announces_one_key() {
        let c = client();
        let t = add_info_only(&c, PURE_V2);
        assert_eq!(c.inner.lsd_info_hashes(), vec![t.info_hash.to_hex()]);
        c.close();
    }
}

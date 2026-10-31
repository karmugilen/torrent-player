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
use wt_types::{InfoHash, PeerId, PeerInfo, PeerSource};

use crate::arrival::ArrivalModel;
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

/// Thin Sources sink / announce adapter over [`TorrentInner`] (agy-2 owns the
/// eventual first-class impls; Client registers torrents immediately).
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
        (info.total_length() - completed).max(0)
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
    pub arrival_model: Arc<RwLock<ArrivalModel>>,
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
            arrival_model: Arc::new(RwLock::new(ArrivalModel::new())),
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
            v2_short: t.info_hash_v2.map(|v| v.to_short()),
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
                let mut aliases = self.torrents_by_short.write();
                if let Some(v1) = spec.info_hash {
                    aliases.insert(v1, Arc::clone(&existing));
                }
                if let Some(v2) = spec.info_hash_v2 {
                    aliases.insert(v2.to_short(), Arc::clone(&existing));
                }
                drop(aliases);
                let mut new_keys = TorrentKeys {
                    v1: None,
                    v2_short: None,
                };
                if spec.info_hash.is_some_and(|v1| v1 != existing.info_hash) {
                    new_keys.v1 = spec.info_hash;
                }
                if let Some(v2) = spec.info_hash_v2 {
                    if existing.info_hash_v2.is_none() && v2.to_short() != existing.info_hash {
                        new_keys.v2_short = Some(v2.to_short());
                    }
                }
                if new_keys.v1.is_some() || new_keys.v2_short.is_some() {
                    let trackers = existing.trackers.read().clone();
                    let web_seeds = existing.web_seeds.read().clone();
                    self.register_sources(&existing, new_keys, trackers, web_seeds);
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
            self.arrival_model.clone(),
        ));
        *torrent_inner.client.write() = Arc::downgrade(self);
        torrent_inner
            .max_established_conns
            .store(self.config.established_conns_per_torrent, Ordering::Relaxed);
        if let Some(info) = info_obj {
            torrent_inner.set_info(Arc::new(info))?;
        }
        if let Some(info_bytes) = spec.info_bytes.clone() {
            *torrent_inner.metainfo_raw.write() = Some(wt_types::MetaInfo {
                info_bytes,
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
                torrent.set_info(Arc::new(info))?;
            }
        }
        if let Some(info_bytes) = &spec.info_bytes {
            *torrent.metainfo_raw.write() = Some(wt_types::MetaInfo {
                info_bytes: info_bytes.clone(),
                ..Default::default()
            });
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

    /// The network changed: map the port on the new router and announce to
    /// the new local network.
    pub fn refresh_local_network(&self) {
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
    fn lsd_info_hashes(&self) -> Vec<String> {
        let torrents: Vec<_> = self.torrents.read().values().cloned().collect();
        torrents.iter()
            .filter(|t| !t.closed.load(Ordering::Relaxed)
                && t.info.read().as_ref().is_some_and(|i| i.private != Some(true)))
            .map(|t| hex::encode(t.info_hash.0))
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
                        let result = cl
                            .dial_peer(Arc::clone(&tor), peer.addr, peer.source)
                            .await;
                        let ok = result.is_ok();
                        if let Err(ref e) = result {
                            let msg = e.to_string();
                            // PopMax already removed the candidate; half-open
                            // races must not permanently discard reachable peers.
                            if msg.contains("half-open limit exceeded") {
                                tor.add_peers(vec![peer.clone()]);
                                tor.dial_finished(peer.addr, true);
                                return;
                            }
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
                // Poll half-open turnover faster than 1s (Go openNewConns is event-driven).
                tokio::select! {
                    _ = torrent.peers_changed().notified() => {},
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {},
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
        let info = AttachInfo {
            remote: accepted.handle.remote_addr(),
            outgoing: false,
            transport: accepted.handle.transport(),
            encrypted: accepted.handle.is_encrypted(),
            peer_id: accepted.peer_id,
            reserved: accepted.peer_extension_bits.0,
            source: PeerSource::Incoming,
        };
        self.attach_driver(
            torrent,
            info,
            accepted.stream,
            accepted.handle,
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
        let reserved = default_peer_extension_bytes();
        let hs_timeout = self.config.handshakes_timeout.max(Duration::from_secs(5));

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

        let (write_tx, write_rx) = mpsc::unbounded_channel();
        let write_waker = Arc::new(Notify::new());
        let closed = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(crate::stats::AtomicConnStats::default());
        let handle = crate::conn::ConnHandle::new(
            remote,
            outgoing,
            wt_types::Transport::WebRtc,
            false,
            torrent.info_hash,
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
        let utp = self
            .listener
            .read()
            .as_ref()
            .and_then(|l| l.utp_socket().cloned());
        let outcome = conn_dial_peer(DialParams {
            remote_addr: addr,
            info_hash: torrent.info_hash,
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

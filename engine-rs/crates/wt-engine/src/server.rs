//! EngineServer: control router, event notification, and subsystem coordinator.
//!
//! Entry point for integration tests and JNI host:
//! - Constructor: `EngineServer::new(ctl_port: u16, download_dir: impl AsRef<Path>, max_peers: usize) -> std::io::Result<Arc<EngineServer>>`
//! - Request entry point: `EngineServer::request(&self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>)`
//!   dispatches any control API route and returns `(http_status_code, response_body_bytes)`.
//! - JNI entry point: implements `crate::jni::host_api::EngineHostApi`.
//!
//! Port of `engine-go/server.go`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::{Condvar, Mutex, RwLock};
use serde_json::json;

use crate::connections::{sort_peer_connections, GeoIpResolver};
use crate::network::NetworkManager;
use crate::playback::{start_stream_server_seekable, PlaybackReader, StreamServer};
use crate::preview::PreviewReader;
use crate::seeding::{SeedAccount, SeedingManager};
use crate::tracker_status::{build_tracker_views, TrackerStatusManager};
use crate::trackers::TrackerPool;
use crate::types::*;

#[path = "metadata_sources.rs"]
mod metadata_sources;
#[path = "torrent_status.rs"]
mod torrent_status;
#[path = "playback_resume.rs"]
mod playback_resume;
#[cfg(test)]
#[path = "server_settings_tests.rs"]
mod settings_tests;

fn new_record_id() -> String {
    use rand::RngCore;
    let mut raw = [0u8; 16];
    if rand::rngs::OsRng.try_fill_bytes(&mut raw).is_err() {
        return format!("torrent-{}", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos());
    }
    raw[6] = (raw[6] & 0x0f) | 0x40;
    raw[8] = (raw[8] & 0x3f) | 0x80;
    use std::fmt::Write;
    let mut hex = String::with_capacity(32);
    for byte in raw { let _ = write!(hex, "{byte:02x}"); }
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

pub struct TorrentRecord {
    pub torrent: wt_core::Torrent,
    pub(crate) playback_coordinator: Arc<crate::playback::PlaybackPriorityCoordinator>,
    playback_demand: HashSet<u32>,
    pub id: String,
    pub info_hash: String,
    pub name: Option<String>,
    pub magnet_uri: Option<String>,
    pub prepared: bool,
    pub configured: bool,
    pub direct_play: bool,
    pub selected: Vec<usize>,
    pub focused_file: Option<usize>,
    pub file_only: bool,
    pub paused: bool,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub checking: bool,
    pub checked_pieces: usize,
    pub check_total: usize,
    pub(crate) verification: Option<Arc<wt_storage::VerificationProgress>>,
    pub files: Vec<FileView>,
    pub original_trackers: Vec<Vec<String>>,
    pub supplemental_trackers: usize,
    pub seeding: bool,
    pub seed_base: SeedAccount,
    pub torrent_bytes: Option<Vec<u8>>,
    pub total_pieces: usize,
    pub piece_length: i64,
    pub last_piece_length: i64,
    pub downloaded: i64,
    pub uploaded: i64,
    pub rates: wt_policy::TransferRates,
    pub verified_bytes: i64,
    pub piece_verified: Vec<bool>,
    pub next_preload: Option<NextPreloadView>,
    pub playback_readers: usize,
    pub resting: bool,
    pub idle_drop_ready: bool,
    /// Go `nextEpisodePreload`; `next_preload` is its status view.
    pub(crate) preload_plan: wt_policy::NextPreload,
    /// Go `reconnectPeers`: dialable addresses kept across network handoffs.
    pub(crate) reconnect_peers: Vec<wt_core::PeerInfo>,
}

impl TorrentRecord {
    pub(crate) fn refresh(&mut self) {
        self.refresh_status();
    }
}

pub struct EngineServer {
    pub(crate) self_ref: OnceLock<Weak<EngineServer>>,
    pub client: wt_core::Client,
    pub storage: Arc<wt_storage::DocumentStorageClient>,
    pub download_dir: PathBuf,
    pub max_peers: AtomicUsize,
    pub ctl_port: u16,
    pub stream_port: AtomicUsize,
    pub stream_server: Mutex<Option<Arc<StreamServer>>>,
    pub trackers: Arc<TrackerPool>,
    pub tracker_status: Arc<TrackerStatusManager>,
    pub geoip: Arc<GeoIpResolver>,
    pub seeding: Arc<SeedingManager>,
    pub network: Arc<NetworkManager>,
    pub records: RwLock<HashMap<String, Arc<RwLock<TorrentRecord>>>>,
    pub records_by_hash: RwLock<HashMap<String, Arc<RwLock<TorrentRecord>>>>,
    event_version: AtomicU64,
    event_cond: Arc<(Mutex<u64>, Condvar)>,
    pub payload_transfers_allowed: AtomicBool,
    pub shutting_down: AtomicBool,
    pub ghost_state: crate::ghost::GhostState,
    pub exploration_state: crate::peer_exploration::ExplorationState,
    pub ghost_mode: AtomicBool,
    pub ghost_pro_mode: AtomicBool,
    pub dial_rate_per_sec: AtomicUsize,
    pub peer_exploration: AtomicBool,
    pub claude_seek_mode: AtomicBool,
    pub seek_stats: Mutex<wt_policy::SeekLatencyStats>,
    pub hedge_requests: AtomicBool,
    pub slow_peer_yield: AtomicBool,
    pub drop_chokers: AtomicBool,
    /// Seek method: deadline order instead of first to ask.
    pub deadline_seek: AtomicBool,
    pub idle_drop_minutes: AtomicI64,
    pub ram_cache_bytes: AtomicI64,
    pub seeding_enabled: AtomicBool,
    pub seeding_allowed: AtomicBool,
    pub seed_ratio_milli: AtomicI64,
    pub seed_time_minutes: AtomicI64,
    pub download_speed: AtomicI64,
    pub upload_speed: AtomicI64,
    pub download_rate_limit: AtomicI64,
    pub upload_rate_limit: AtomicI64,
    pub preview_active: Mutex<Option<Arc<PreviewReader>>>,
    pub preview_active_id: Mutex<Option<String>>,
    pub preview_readers: Mutex<HashMap<i64, Arc<PreviewReader>>>,
    pub playback_readers: Mutex<HashMap<i64, Arc<PlaybackReader>>>,
    resume_state: Mutex<playback_resume::ResumeState>,
    pub next_reader_id: AtomicI64,
    pub bypass_enabled: AtomicBool,
    pub(crate) idle_states: crate::startup_policy::IdleStates,
    /// Go `peerHints`: peers that completed a handshake, per torrent.
    pub(crate) peer_hints: Arc<Mutex<wt_storage::PeerHintStore>>,
}

impl EngineServer {
    pub fn new(
        ctl_port: u16,
        download_dir: impl AsRef<Path>,
        max_peers: usize,
    ) -> std::io::Result<Arc<Self>> {
        Self::new_with_config(ctl_port, download_dir, max_peers, |_| {})
    }

    /// Apply host-specific transport options before starting the core client.
    pub fn new_with_config(
        ctl_port: u16,
        download_dir: impl AsRef<Path>,
        max_peers: usize,
        customize: impl FnOnce(&mut wt_core::ClientConfig),
    ) -> std::io::Result<Arc<Self>> {
        let dir = download_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;

        let trackers = Arc::new(TrackerPool::new(&dir));
        let tracker_status = Arc::new(TrackerStatusManager::new());
        let geoip = Arc::new(GeoIpResolver::new(&dir));
        let seeding = Arc::new(SeedingManager::new(&dir));
        let network = Arc::new(NetworkManager::new());

        let effective_peers = if max_peers == 0 {
            100
        } else {
            max_peers.min(500)
        };
        let (half_open_per, half_open_total) = wt_core::half_open_for_max_peers(effective_peers);

        let storage = Arc::new(wt_storage::DocumentStorageClient::new(&dir));
        let status_target = tracker_status.clone();
        let client_ref: Arc<OnceLock<Weak<wt_core::ClientInner>>> = Arc::new(OnceLock::new());
        let callback_client = client_ref.clone();
        let hint_dir = dir.clone();
        let hint_store = Arc::new(Mutex::new(wt_storage::PeerHintStore::new()));
        hint_store.lock().load(&dir);
        let callback_hints = hint_store.clone();
        let callbacks = wt_core::Callbacks {
            status_updated: vec![Arc::new(move |event| {
                let (hash, url, status, message) = match event {
                    wt_core::StatusEvent::TrackerConnected { info_hash, url }
                    | wt_core::StatusEvent::TrackerAnnounceSuccessful { info_hash, url } => {
                        (info_hash, url, "working", "")
                    }
                    wt_core::StatusEvent::TrackerDisconnected {
                        info_hash,
                        url,
                        error,
                    } => (info_hash, url, "error", error.as_deref().unwrap_or("")),
                    wt_core::StatusEvent::TrackerAnnounceError {
                        info_hash,
                        url,
                        error,
                    } => (info_hash, url, "error", error.as_str()),
                };
                status_target.set_status(&hash.to_hex(), url, status, message);
            })],
            completed_handshake: vec![Arc::new(move |connection, hash| {
                let Some(inner) = callback_client.get().and_then(Weak::upgrade) else {
                    return;
                };
                let Some(info) = (wt_core::Client { inner })
                    .torrent(hash)
                    .and_then(|t| t.info())
                else {
                    return;
                };
                if info.private == Some(true) {
                    return;
                }
                let peer = wt_core::PeerInfo {
                    id: connection.peer_id,
                    addr: connection.dial_addr.unwrap_or(connection.remote_addr),
                    source: connection.source,
                    supports_encryption: connection.encrypted,
                    pex: Default::default(),
                    trusted: false,
                };
                let hints = callback_hints.clone();
                let dir = hint_dir.clone();
                let save = move || {
                    hints
                        .lock()
                        .remember_peer_hint(&dir, &hash.to_hex(), &peer, false)
                };
                match tokio::runtime::Handle::try_current() {
                    Ok(runtime) => {
                        runtime.spawn_blocking(save);
                    }
                    Err(_) => {
                        std::thread::spawn(save);
                    }
                }
            })],
        };
        let dialer = network.dialer();
        let tracker_dialer = Some(Arc::new(
            move |_network: String,
                  address: String|
                  -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = std::io::Result<tokio::net::TcpStream>> + Send,
                >,
            > {
                let dialer = dialer.clone();
                Box::pin(async move {
                    let epoch = dialer.epoch();
                    let stream = tokio::time::timeout(
                        Duration::from_secs(6),
                        tokio::net::TcpStream::connect(address),
                    )
                    .await
                    .map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "tracker dial timed out")
                    })??;
                    if dialer.epoch() != epoch {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::ConnectionAborted,
                            "network changed",
                        ));
                    }
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        dialer.track_fd(stream.as_raw_fd());
                    }
                    Ok(stream)
                })
            },
        ) as _);
        let mut config = wt_core::ClientConfig {
            callbacks,
            tracker_dialer,
            header_obfuscation_policy: Default::default(),
            default_storage: storage.clone(),
            listen_port: 0,
            no_dht: false,
            disable_utp: false,
            disable_tcp: false,
            disable_webtorrent: false,
            disable_pex: false,
            dial_for_peer_conns: true,
            accept_peer_connections: true,
            download_rate: None,
            upload_rate: None,
            seed: true,
            always_want_conns: true,
            periodically_announce_torrents_to_dht: true,
            piece_hashers_per_torrent: wt_storage::background_workers(),
            dial_rate: 30.0,
            nominal_dial_timeout: Duration::from_secs(8),
            min_dial_timeout: Duration::from_secs(2),
            handshakes_timeout: Duration::from_secs(5),
            established_conns_per_torrent: effective_peers,
            half_open_conns_per_torrent: half_open_per,
            total_half_open_conns: half_open_total,
            torrent_peers_high_water: 120.max(effective_peers * 4),
            torrent_peers_low_water: 40.max(effective_peers * 2),
            http_user_agent: format!("TorrentPlayer/{}", crate::version::app_version()),
            port_forwarding: true,
            local_service_discovery: true,
            extended_handshake_client_version: format!(
                "Torrent Player {}",
                crate::version::app_version()
            ),
            ice_servers: [
                "stun:stun.l.google.com:19302",
                "stun:stun1.l.google.com:19302",
                "stun:stun2.l.google.com:19302",
                "stun:stun3.l.google.com:19302",
                "stun:stun4.l.google.com:19302",
                "stun:global.stun.twilio.com:3478",
                "stun:stun.cloudflare.com:3478",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        };
        customize(&mut config);
        let client = wt_core::Client::new(config).map_err(std::io::Error::other)?;
        let _ = client_ref.set(Arc::downgrade(&client.inner));
        let mut contacts = wt_storage::DhtContactStore::new();
        contacts.load(&dir);
        client.add_dht_nodes(
            &contacts
                .contacts()
                .iter()
                .map(|c| c.addr.clone())
                .collect::<Vec<_>>(),
        );
        let server = Arc::new(Self {
            self_ref: OnceLock::new(),
            client,
            storage,
            download_dir: dir,
            max_peers: AtomicUsize::new(effective_peers),
            ctl_port,
            stream_port: AtomicUsize::new(0),
            stream_server: Mutex::new(None),
            trackers,
            tracker_status,
            geoip,
            seeding,
            network,
            records: RwLock::new(HashMap::new()),
            records_by_hash: RwLock::new(HashMap::new()),
            event_version: AtomicU64::new(1),
            event_cond: Arc::new((Mutex::new(1), Condvar::new())),
            payload_transfers_allowed: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
            ghost_state: Default::default(),
            exploration_state: Default::default(),
            ghost_mode: AtomicBool::new(false),
            ghost_pro_mode: AtomicBool::new(false),
            dial_rate_per_sec: AtomicUsize::new(30),
            peer_exploration: AtomicBool::new(false),
            claude_seek_mode: AtomicBool::new(false),
            seek_stats: Mutex::new(wt_policy::SeekLatencyStats::default()),
            hedge_requests: AtomicBool::new(true),
            slow_peer_yield: AtomicBool::new(true),
            drop_chokers: AtomicBool::new(true),
            deadline_seek: AtomicBool::new(false),
            idle_drop_minutes: AtomicI64::new(0),
            ram_cache_bytes: AtomicI64::new(64 * 1024 * 1024),
            seeding_enabled: AtomicBool::new(false),
            seeding_allowed: AtomicBool::new(true),
            seed_ratio_milli: AtomicI64::new(1000),
            seed_time_minutes: AtomicI64::new(60),
            download_speed: AtomicI64::new(0),
            upload_speed: AtomicI64::new(0),
            download_rate_limit: AtomicI64::new(0),
            upload_rate_limit: AtomicI64::new(0),
            preview_active: Mutex::new(None),
            preview_active_id: Mutex::new(None),
            preview_readers: Mutex::new(HashMap::new()),
            playback_readers: Mutex::new(HashMap::new()),
            resume_state: Mutex::new(playback_resume::ResumeState::default()),
            next_reader_id: AtomicI64::new(1),
            bypass_enabled: AtomicBool::new(false),
            idle_states: Default::default(),
            peer_hints: hint_store,
        });
        let _ = server.self_ref.set(Arc::downgrade(&server));
        let weak = Arc::downgrade(&server);
        server.tracker_status.on_change(Arc::new(move || {
            if let Some(server) = weak.upgrade() { server.notify_change(); }
        }));
        let weak = Arc::downgrade(&server);
        server.trackers.start(&server.client.inner.tokio_handle,
            crate::trackers::TRACKER_LIST_SOURCE.to_string(), Arc::new(move || {
                if let Some(server) = weak.upgrade() { server.refresh_torrent_trackers(); }
            }));
        Self::start_policy_sampler(&server);
        // Match Go: defaults (hedge/slowPeerYield/learnedArrival) must reach
        // wt-core SeekTuning before the first torrent, not only after POST /settings.
        server.apply_seek_tuning();
        Ok(server)
    }

    fn apply_seek_tuning(&self) {
        wt_core::set_seek_tuning(wt_core::SeekTuning {
            hedge: self.hedge_requests.load(Ordering::SeqCst),
            slow_peer_yield: self.slow_peer_yield.load(Ordering::SeqCst),
            deadline: self.deadline_seek.load(Ordering::SeqCst),
        });
    }

    /// Go `recoverNetwork`: close the old route's peers (keeping up to 80
    /// dialable addresses), then re-add them and re-announce to trackers and
    /// DHT at 0, 3, 10 and 30 s until each torrent has active peers again.
    fn recover_network(&self, episode: u64) {
        let current = || self.network.episode() == episode
            && !self.shutting_down.load(Ordering::SeqCst);
        let records: Vec<_> = self.records.read().values().cloned().collect();
        for rec_arc in &records {
            let mut rec = rec_arc.write();
            if !current() || rec.torrent.closed() {
                continue;
            }
            rec.torrent.route_changed();
            if rec.paused {
                continue;
            }
            let mut seen = HashSet::new();
            let mut cached = Vec::new();
            let conns = rec.torrent.peer_conns();
            let previous = std::mem::take(&mut rec.reconnect_peers);
            let candidates = conns.iter().map(|pc| wt_core::PeerInfo {
                id: None,
                addr: pc.remote_addr,
                source: pc.source,
                supports_encryption: pc.encrypted,
                pex: Default::default(),
                trusted: false,
            }).chain(previous);
            for peer in candidates {
                if cached.len() >= 80 || peer.addr.port() == 0 || !seen.insert(peer.addr) {
                    continue;
                }
                cached.push(peer);
            }
            rec.torrent.close_all_peers("network changed");
            // Reconnect widely on the new route, then keep the best peers.
            rec.torrent.start_peer_burst();
            rec.reconnect_peers = cached;
        }
        self.client.network_changed();
        self.notify_change();
        for delay in [0u64, 3, 10, 30] {
            let until = Instant::now() + Duration::from_secs(delay);
            while Instant::now() < until {
                if !current() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            if !current() {
                return;
            }
            let mut pending = false;
            for rec_arc in &records {
                let rec = rec_arc.read();
                if rec.paused || rec.torrent.closed()
                    || (delay != 0 && rec.torrent.stats().active_peers > 0)
                {
                    continue;
                }
                pending = true;
                rec.torrent.add_peers(rec.reconnect_peers.clone());
                let torrent = rec.torrent.clone();
                drop(rec);
                torrent.rediscover();
            }
            if !pending {
                return;
            }
            self.notify_change();
        }
    }

    /// Go `applyPeerHints`: add unexpired remembered peers of a public torrent.
    fn apply_peer_hints(&self, rec: &TorrentRecord) {
        let Some(info) = rec.torrent.info() else { return; };
        if info.private == Some(true) {
            return;
        }
        let hints = self.peer_hints.lock().get_hints(&rec.info_hash);
        let now = std::time::SystemTime::now();
        let peers: Vec<wt_core::PeerInfo> = hints.into_iter()
            .filter(|h| wt_storage::parse_time(&h.expires).is_some_and(|t| t > now))
            .filter_map(|h| {
                let addr = h.addr.parse::<std::net::SocketAddr>().ok()?;
                let source = [
                    wt_core::PeerSource::Tracker, wt_core::PeerSource::Incoming,
                    wt_core::PeerSource::DhtGetPeers, wt_core::PeerSource::DhtAnnouncePeer,
                    wt_core::PeerSource::Pex, wt_core::PeerSource::UtHolepunch,
                    wt_core::PeerSource::WebRtc, wt_core::PeerSource::WebSeed,
                    wt_core::PeerSource::Lsd,
                ].into_iter().find(|s| s.code() == h.source).unwrap_or(wt_core::PeerSource::Direct);
                Some(wt_core::PeerInfo {
                    id: None,
                    addr,
                    source,
                    supports_encryption: false,
                    pex: Default::default(),
                    trusted: false,
                })
            })
            .collect();
        if !peers.is_empty() {
            rec.torrent.add_peers(peers);
        }
    }

    pub fn close_preview_for(&self, id: &str) {
        let mut active_id = self.preview_active_id.lock();
        if active_id.as_deref() == Some(id) {
            *active_id = None;
            let p = self.preview_active.lock().take();
            if let Some(reader) = p {
                reader.close();
            }
        }
    }

    pub fn status_fingerprint(&self) -> [u8; 32] {
        self.telemetry_fingerprint()
    }

    pub fn record_by_id(&self, id: &str) -> Option<Arc<RwLock<TorrentRecord>>> {
        self.records.read().get(id).cloned()
    }

    /// JNI requests are bounded to 60 seconds, as in Go waitForInfo.
    /// Call without holding records or record locks so removal can wake the waiter.
    fn wait_for_info(&self, torrent: &wt_core::Torrent) -> Result<(), String> {
        if torrent.closed() {
            return Err("torrent removed".into());
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err("torrent is closing".into());
        }
        torrent.wait_info(Some(Duration::from_secs(60))).map_err(|e| {
            if torrent.closed() { "torrent removed".into() } else { e.to_string() }
        })?;
        if torrent.closed() {
            return Err("torrent removed".into());
        }
        Ok(())
    }

    pub fn storage_for_torrent(
        &self,
        torrent: &wt_core::Torrent,
    ) -> Option<Arc<wt_storage::DocumentTorrentStorage>> {
        let hex = torrent.info_hash().to_hex();
        if let Some(st) = self.storage.get_storage(&hex) {
            return Some(st);
        }
        if let Some(info) = torrent.info() {
            if torrent.num_pieces() > 0 {
                use wt_types::ClientStorage;
                let _ = self.storage.open_torrent(&info, torrent.info_hash());
            }
        }
        self.storage.get_storage(&hex)
    }

    fn apply_seed_policy(&self, rec: &mut TorrentRecord) {
        self.apply_seeding_policy(rec, false);
    }

    fn apply_transfer_state(&self, rec: &mut TorrentRecord) {
        rec.torrent.inner.saved_mode.store(!rec.direct_play, Ordering::Relaxed);
        if let Some(progress) = &rec.verification {
            progress.set_paused(rec.paused || !self.payload_transfers_allowed.load(Ordering::SeqCst));
        }
        let active = self.payload_transfers_allowed.load(Ordering::SeqCst)
            && rec.configured
            && !rec.paused
            && !rec.checking
            && !rec.torrent.closed()
            && rec.error.is_none();
        if !active {
            rec.torrent.disallow_data_download();
            rec.torrent.disallow_data_upload();
            rec.playback_coordinator.hold_resume(HashMap::new());
            // Cancellation only touches the token, never a session lock that
            // an in-flight read holds while calling back into this record.
            for reader in self.playback_readers.lock().values().filter(|r| r.id == rec.id) {
                reader.cancel_pending_read();
            }
        }
        if let Some(info) = rec.torrent.info() {
            let only = rec.file_only && rec.focused_file.is_some() && !rec.direct_play;
            let mut priorities = vec![wt_core::PiecePriority::None; rec.torrent.num_pieces() as usize];
            if active && !rec.direct_play && info.piece_length > 0 {
                for file in rec.torrent.files().iter().filter(|file|
                    rec.selected.contains(&file.index()) && (!only || Some(file.index()) == rec.focused_file)) {
                    let begin = (file.offset() / info.piece_length) as usize;
                    let end = ((file.offset() + file.length() + info.piece_length - 1) / info.piece_length) as usize;
                    // File boundaries can share a piece. An unselected neighbor
                    // must never overwrite a selected file's boundary demand.
                    for priority in priorities.iter_mut().take(end).skip(begin) {
                        *priority = wt_core::PiecePriority::Normal;
                    }
                }
            }
            rec.torrent.set_piece_priorities(priorities.into_iter().enumerate()
                .map(|(piece, priority)| (piece as u32, priority)));
            // Soft head/tail Now hints on top of the base Normal priority;
            // gated off (cleared) while inactive.
            self.apply_video_startup_hints(rec);
            self.apply_background_focus(rec);
            if active {
                let demand = rec.playback_coordinator.effective();
                Self::apply_record_piece_priorities(rec, &demand);
                rec.torrent.allow_data_download();
                rec.torrent.allow_data_upload();
            }
        }
        self.apply_seed_policy(rec);
    }

    pub(crate) fn apply_record_piece_priorities(
        rec: &mut TorrentRecord,
        demand: &HashMap<u32, wt_core::PiecePriority>,
    ) {
        let previous = std::mem::take(&mut rec.playback_demand);
        let info = rec.torrent.info();
        let files = rec.torrent.files();
        let mut priorities = Vec::with_capacity(previous.len() + demand.len());
        for piece in previous {
            if demand.contains_key(&piece) { continue; }
            let wanted = !rec.direct_play && info.as_ref().is_some_and(|info| {
                let p = info.piece(piece);
                files.iter().any(|f| rec.selected.contains(&f.index())
                    && (!rec.file_only || rec.focused_file.is_none() || rec.focused_file == Some(f.index()))
                    && f.offset() < p.offset + p.length
                    && f.offset() + f.length() > p.offset)
            });
            priorities.push((piece, if wanted {
                wt_core::PiecePriority::Normal
            } else { wt_core::PiecePriority::None }));
        }
        priorities.extend(demand.iter().map(|(&piece, &priority)| (piece, priority)));
        rec.torrent.set_piece_priorities(priorities);
        rec.playback_demand.extend(demand.keys().copied());
    }

    /// Go `watchMetadata`: a magnet has no info at add time, so the public
    /// tracker supplement runs once its metadata arrives.
    fn watch_metadata(&self, torrent: &wt_core::Torrent, rec: &Arc<RwLock<TorrentRecord>>) {
        if torrent.info().is_some() {
            return;
        }
        let Some(server) = self.self_ref.get().cloned() else {
            return;
        };
        let torrent = torrent.clone();
        let rec = rec.clone();
        self.client.inner.tokio_handle.spawn_blocking(move || loop {
            match torrent.wait_info(Some(Duration::from_secs(5))) {
                Ok(_) => {
                    if let Some(server) = server.upgrade() {
                        server.supplement_trackers(&rec);
                        server.notify_change();
                    }
                    return;
                }
                Err(wt_core::CoreError::Closed) => return,
                Err(_) => match server.upgrade() {
                    Some(s) if !s.shutting_down.load(Ordering::SeqCst) => {}
                    _ => return,
                },
            }
        });
    }

    pub fn notify_change(&self) {
        let (lock, cvar) = &*self.event_cond;
        let mut ver = lock.lock();
        *ver = ver.wrapping_add(1);
        self.event_version.store(*ver, Ordering::SeqCst);
        cvar.notify_all();
    }

    pub fn wait_for_change(&self, after: u64, timeout: Duration) -> u64 {
        let (lock, cvar) = &*self.event_cond;
        let mut ver = lock.lock();
        if after == u64::MAX {
            return *ver;
        }
        if *ver > after {
            return *ver;
        }
        let deadline = Instant::now() + timeout;
        while *ver <= after {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let res = cvar.wait_for(&mut ver, deadline - now);
            if res.timed_out() {
                break;
            }
        }
        *ver
    }

    pub async fn start_streaming(self: &Arc<Self>) -> std::io::Result<()> {
        let this = self.clone();
        let resolver =
            Arc::new(move |id: &str, file_index: usize, start: i64| this.open_playback_at(id, file_index, start));

        let server = start_stream_server_seekable(0, resolver).await?;
        self.stream_port
            .store(server.port() as usize, Ordering::SeqCst);
        *self.stream_server.lock() = Some(server);
        self.notify_change();
        Ok(())
    }

    pub fn close(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        for record in self.records.read().values() {
            if let Some(progress) = &record.read().verification { progress.cancel(); }
        }
        let mut contacts = wt_storage::DhtContactStore::new();
        contacts.load(&self.download_dir);
        for addr in self.client.dht_contacts() {
            contacts.remember_dht_contact(&self.download_dir, &addr.to_string(), None);
        }
        self.trackers.close();
        self.seed_tick();
        self.seeding.save();
        if let Some(server) = self.stream_server.lock().take() {
            server.close();
        }
        let readers = std::mem::take(&mut *self.playback_readers.lock());
        for (_, r) in readers {
            r.close();
        }
        self.flush_resume_saves();
        self.client.close();
        let preview = self.preview_active.lock().take();
        if let Some(p) = preview {
            p.close();
        }
        *self.preview_active_id.lock() = None;
        self.notify_change();
    }

    pub fn dispatch_control(&self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return (
                503,
                serde_json::to_vec(&json!({"error": "engine is shutting down"})).unwrap(),
            );
        }

        let clean_path = path.trim_start_matches('/');
        let (endpoint, query) = if let Some((ep, q)) = clean_path.split_once('?') {
            (ep, q)
        } else {
            (clean_path, "")
        };
        let parts: Vec<&str> = endpoint.split('/').collect();

        let response = match (method, parts.as_slice()) {
            ("GET", ["stats"]) => self.handle_stats(),
            ("GET", ["events"]) => self.handle_events(),
            ("GET", ["version"]) => (
                200,
                serde_json::to_vec(&json!({"version": crate::version::app_version()})).unwrap(),
            ),
            ("POST", ["add"]) => self.handle_add(body),
            ("GET", ["torrent", id]) => self.handle_get_torrent(id),
            ("POST", ["play"]) => self.handle_play(body),
            ("POST", ["file-only"]) => self.handle_file_only(body),
            ("POST", ["focus"]) => self.handle_focus(body),
            ("POST", ["configure"]) => self.handle_configure(body),
            ("POST", ["select"]) => self.handle_select(body),
            ("POST", ["pause"]) => self.handle_pause_resume(body, true),
            ("POST", ["resume"]) => self.handle_pause_resume(body, false),
            ("POST", ["remove"]) => self.handle_remove(body),
            ("POST", ["forget-peers"]) => self.handle_forget_peers(body),
            ("GET", ["pieces", id]) => self.handle_pieces(id, query),
            ("GET", ["preview-state", id]) => self.handle_preview_state(id, query),
            ("GET", ["connections", id]) => self.handle_connections(id),
            ("GET", ["metadata", id]) => self.handle_metadata(id),
            ("GET", ["settings"]) => self.handle_get_settings(),
            ("POST", ["settings"]) => self.handle_post_settings(body),
            ("POST", ["network"]) => self.handle_network_change(body),
            ("GET", ["bypass"]) => self.handle_get_bypass(),
            ("POST", ["bypass"]) => self.handle_post_bypass(body),
            ("GET", ["cache"]) => self.handle_get_cache(),
            ("POST", ["cache", "clear"]) => self.handle_clear_cache(body),
            ("POST", ["shutdown"]) => {
                self.close();
                (
                    200,
                    serde_json::to_vec(&OkResponse { ok: true, id: None }).unwrap(),
                )
            }
            _ => (
                404,
                serde_json::to_vec(&json!({"error": "not found"})).unwrap(),
            ),
        };
        if response.0 == 200 && method == "POST"
            && matches!(parts.first().copied(), Some("configure" | "select" | "play" | "pause" | "resume" | "remove" | "network" | "file-only" | "focus"))
        {
            self.reset_ghost();
        }
        response
    }

    /// Recent engine events and per-torrent totals, for diagnostics.
    fn handle_events(&self) -> (u16, Vec<u8>) {
        let events: Vec<_> = wt_types::events::snapshot().into_iter()
            .map(|(at, kind, detail)| json!({"at": at, "kind": kind, "detail": detail}))
            .collect();
        let torrents: Vec<_> = self.records.read().values()
            .map(|r| {
                let rec = r.read();
                json!({"id": rec.id, "stats": format!("{:?}", rec.torrent.stats())})
            })
            .collect();
        let seek = format!("{:?}", self.seek_stats.lock().snapshot());
        (200, serde_json::to_vec(&json!({"events": events, "torrents": torrents, "seek": seek})).unwrap())
    }

    fn handle_stats(&self) -> (u16, Vec<u8>) {
        let torrents = self.records.read().len();
        let resp = StatsResponse {
            download_speed: self.download_speed.load(Ordering::SeqCst),
            upload_speed: self.upload_speed.load(Ordering::SeqCst),
            progress: 0.0,
            ratio: 0.0,
            torrents,
            ctl_port: self.ctl_port,
            stream_port: self.stream_port.load(Ordering::SeqCst) as u16,
            path: self.download_dir.to_string_lossy().to_string(),
            payload_transfers_allowed: self.payload_transfers_allowed.load(Ordering::SeqCst),
            listen_port: self.client.local_port(),
        };
        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_add(&self, body: &str) -> (u16, Vec<u8>) {
        let mut value: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":"invalid json"})).unwrap(),
                )
            }
        };
        if let Some(obj) = value.as_object_mut() {
            obj.entry("torrentId").or_insert(json!(""));
        }
        let req: AddRequest = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                );
            }
        };

        let mut torrent_data_bytes = None;
        if let Some(b64) = req.torrent_data.as_deref().filter(|data| !data.is_empty()) {
            // Go: damaged data is an error, not a fall back to torrentId.
            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64) {
                Ok(bytes) => torrent_data_bytes = Some(bytes),
                Err(_) => {
                    return (
                        400,
                        serde_json::to_vec(&json!({"error": "invalid torrentData base64"})).unwrap(),
                    )
                }
            }
        }

        let mut spec = wt_core::TorrentSpec::default();
        let mut peers = Vec::new();
        let mut metadata_sources = Vec::new();
        if let Some(ref bytes) = torrent_data_bytes {
            let mi = match wt_meta::parse_metainfo(bytes) {
                Ok(mi) => mi,
                Err(e) => {
                    return (
                        400,
                        serde_json::to_vec(&json!({"error": e.to_string()})).unwrap(),
                    )
                }
            };
            spec.info_hash = Some(wt_meta::info_hash_v1(&mi.info_bytes));
            spec.info_bytes = Some(mi.info_bytes);
            spec.trackers = mi.announce_list;
            if spec.trackers.is_empty() && !mi.announce.is_empty() {
                spec.trackers.push(vec![mi.announce]);
            }
            spec.web_seeds = mi.url_list;
            spec.dht_nodes = mi.nodes;
            spec.piece_layers = mi.piece_layers;
        } else if req.torrent_id.starts_with("magnet:") {
            let m = match wt_meta::parse_magnet(&req.torrent_id) {
                Ok(m) => m,
                Err(e) => {
                    return (
                        400,
                        serde_json::to_vec(&json!({"error": e.to_string()})).unwrap(),
                    )
                }
            };
            spec.info_hash = m.info_hash;
            spec.info_hash_v2 = m.info_hash_v2;
            spec.display_name = m.display_name;
            spec.trackers = vec![m.trackers];
            // xs/as are metainfo URLs, distinct from BEP 19 ws payload URLs.
            // Read these from the URI because wt-meta currently folds them into web_seeds.
            let query = req.torrent_id.split_once('?').map_or("", |(_, query)| query);
            let params: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
            spec.web_seeds = params.iter().filter(|(key, _)| key == "ws")
                .map(|(_, value)| value.to_string()).collect();
            metadata_sources = ["xs", "as"].into_iter().flat_map(|key| {
                params.iter().filter(move |(k, _)| k == key).map(|(_, v)| v.to_string())
            }).collect();
            peers = m
                .params
                .into_iter()
                .filter_map(|(k, v)| if k == "x.pe" { v.parse().ok() } else { None })
                .collect();
        } else {
            spec.info_hash = wt_core::InfoHash::from_hex(&req.torrent_id);
            if spec.info_hash.is_none() {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":"invalid torrent"})).unwrap(),
                );
            }
        }
        if let Some(hash) = spec.info_hash {
            if let Some(existing) = self.records_by_hash.read().get(&hash.to_hex()) {
                return (200, serde_json::to_vec(&AddResponse {
                    id: existing.read().id.clone(), info_hash: Some(hash.to_hex()),
                }).unwrap());
            }
        }
        spec.trackers = crate::trackers::clean_supplied_trackers(&spec.trackers);
        let original_trackers = spec.trackers.clone();
        let (torrent, _) = match self.client.add_torrent_spec(spec) {
            Ok(t) => t,
            Err(e) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":e.to_string()})).unwrap(),
                )
            }
        };
        torrent.add_peers(
            peers
                .into_iter()
                .map(|addr| wt_core::PeerInfo {
                    id: None,
                    addr,
                    source: wt_core::PeerSource::Direct,
                    supports_encryption: false,
                    pex: Default::default(),
                    trusted: false,
                })
                .collect(),
        );
        torrent.set_max_established_conns(self.max_peers.load(Ordering::SeqCst));
        let info_hash = torrent.info_hash().to_hex();
        let id = new_record_id();
        let info = torrent.info();
        let files: Vec<FileView> = torrent
            .files()
            .into_iter()
            .map(|file| {
                let path = file.path();
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                FileView {
                    index: file.index(),
                    name: name.clone(),
                    path,
                    length: file.length(),
                    progress: 0.0,
                    verified_bytes: file.bytes_completed(),
                    file_type: crate::stream_http::content_type_for_name(&name).to_string(),
                }
            })
            .collect();
        let total_pieces = torrent.num_pieces() as usize;
        let piece_length = info.as_ref().map_or(0, |i| i.piece_length);
        let last_piece_length = info.as_ref().map_or(0, |i| {
            if total_pieces == 0 {
                0
            } else {
                i.piece((total_pieces - 1) as u32).length
            }
        });
        let mut magnet = url::Url::parse(&format!("magnet:?xt=urn:btih:{info_hash}")).unwrap();
        for tracker in original_trackers.iter().flatten() {
            magnet.query_pairs_mut().append_pair("tr", tracker);
        }
        let rec = Arc::new(RwLock::new(TorrentRecord {
            torrent: torrent.clone(),
            playback_coordinator: Arc::new(crate::playback::PlaybackPriorityCoordinator::new()),
            playback_demand: HashSet::new(),
            id: id.clone(),
            info_hash: info_hash.clone(),
            name: info.as_ref().map(|i| i.name.clone()),
            magnet_uri: Some(magnet.to_string()),
            prepared: req.prepare,
            configured: false,
            direct_play: false,
            selected: Vec::new(),
            focused_file: None,
            file_only: false,
            paused: false,
            error: None,
            warning: None,
            checking: false,
            checked_pieces: 0,
            check_total: 10,
            verification: None,
            files,
            original_trackers,
            supplemental_trackers: 0,
            seeding: false,
            seed_base: self.seeding.get_account(&info_hash),
            torrent_bytes: torrent_data_bytes,
            total_pieces,
            piece_length,
            last_piece_length,
            downloaded: 0,
            uploaded: 0,
            rates: {
                let mut rates = wt_policy::TransferRates::default();
                rates.sample(Instant::now(), 0, 0, false);
                rates
            },
            verified_bytes: 0,
            piece_verified: vec![false; total_pieces],
            next_preload: None,
            playback_readers: 0,
            resting: false,
            idle_drop_ready: false,
            preload_plan: Default::default(),
            reconnect_peers: Vec::new(),
        }));

        {
            // Match remove's lock order and coalesce concurrent additions of one hash.
            let mut records = self.records.write();
            let mut by_hash = self.records_by_hash.write();
            if let Some(existing) = by_hash.get(&info_hash) {
                return (200, serde_json::to_vec(&AddResponse {
                    id: existing.read().id.clone(), info_hash: Some(info_hash),
                }).unwrap());
            }
            records.insert(id.clone(), rec.clone());
            by_hash.insert(info_hash.clone(), rec.clone());
        }
        self.supplement_trackers(&rec);
        self.watch_metadata(&torrent, &rec);
        self.start_metadata_sources(&torrent, metadata_sources);
        self.notify_change();

        (
            200,
            serde_json::to_vec(&AddResponse {
                id,
                info_hash: Some(info_hash),
            })
            .unwrap(),
        )
    }

    fn handle_get_torrent(&self, id: &str) -> (u16, Vec<u8>) {
        let records = self.records.read();
        let rec = match records.get(id) {
            Some(r) => {
                let mut rec = r.write();
                rec.refresh();
                self.apply_seed_policy(&mut rec);
                rec
            }
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                );
            }
        };

        let live_status = self.tracker_status.snapshot(&rec.info_hash);
        let trackers = build_tracker_views(&rec.original_trackers, &rec.torrent.inner.trackers.read(), &live_status);

        let selected_total: i64 = rec
            .files
            .iter()
            .filter(|f| rec.selected.contains(&f.index))
            .map(|f| f.length)
            .sum();
        let selected_verified: i64 = rec
            .files
            .iter()
            .filter(|f| rec.selected.contains(&f.index))
            .map(|f| f.verified_bytes)
            .sum();
        let selected_received: i64 = rec.files.iter()
            .filter(|f| rec.selected.contains(&f.index))
            .map(|f| if f.length > 0 { (f.progress * f.length as f64).round() as i64 } else { 0 })
            .sum();
        let down_speed = if rec.paused { 0 } else { rec.rates.down };
        let up_speed = if rec.paused { 0 } else { rec.rates.up };
        let view = TorrentView {
            seeding: rec.seeding,
            seed_ratio: 0.0,
            id: rec.id.clone(),
            info_hash: Some(rec.info_hash.clone()),
            name: rec.name.clone(),
            magnet_uri: rec.magnet_uri.clone(),
            ready: rec.torrent.info().is_some(),
            done: !rec.direct_play
                && !rec.checking
                && selected_total > 0
                && selected_verified >= selected_total,
            paused: rec.paused,
            progress: if selected_total > 0 {
                selected_received as f64 / selected_total as f64
            } else {
                0.0
            },
            download_speed: down_speed,
            upload_speed: up_speed,
            num_peers: rec.torrent.stats().total_peers,
            length: selected_total,
            downloaded: selected_received,
            verified_bytes: selected_verified,
            uploaded: rec.uploaded,
            files: rec.files.clone(),
            trackers,
            error: rec.error.clone(),
            time_remaining: if !rec.direct_play && down_speed > 0 && selected_total > selected_received {
                Some(((selected_total - selected_received) as f64 / down_speed as f64 * 1000.0) as i64)
            } else { None },
            configured: rec.configured,
            selected: rec.selected.clone(),
            focused_file: rec.focused_file,
            checking: rec.checking,
            checked_pieces: rec.checked_pieces,
            check_total: rec.check_total,
            warning: rec.warning.clone(),
            playback_readers: rec.playback_readers,
            resting: rec.resting,
            idle_drop_ready: rec.idle_drop_ready,
            next_preload: Self::next_preload_view(&rec),
        };

        (200, serde_json::to_vec(&view).unwrap())
    }

    fn handle_play(&self, body: &str) -> (u16, Vec<u8>) {
        let mut value: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":"invalid json"})).unwrap(),
                )
            }
        };
        if let Some(obj) = value.as_object_mut() {
            obj.entry("id").or_insert(json!(""));
        }
        let req: PlayRequest = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };

        let (rec_arc, torrent) = match self.record_by_id(&req.id) {
            Some(r) => {
                let rec = r.read();
                (r.clone(), rec.torrent.clone())
            }
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                );
            }
        };

        if let Err(error) = self.wait_for_info(&torrent) {
            return (409, serde_json::to_vec(&json!({"error": error})).unwrap());
        }

        let mut rec = rec_arc.write();
        if torrent.closed() {
            return (409, serde_json::to_vec(&json!({"error": "torrent removed"})).unwrap());
        }
        rec.refresh();

        if !rec.configured {
            return (
                409,
                serde_json::to_vec(&json!({"error": "download storage is not configured"}))
                    .unwrap(),
            );
        }

        let file_index = match req.file_index.or_else(|| {
            rec.selected.iter().copied().filter(|&i| i < rec.files.len())
                .max_by_key(|&i| rec.files[i].length)
        }) {
            Some(index) if index < rec.files.len() && rec.selected.contains(&index) => index,
            _ => return (400, serde_json::to_vec(&json!({"error": "file is not selected"})).unwrap()),
        };
        let file = rec.files[file_index].clone();
        if file.verified_bytes < file.length {
            // An incomplete stream would appear to start, then hang waiting
            // for bytes this policy deliberately blocks.
            if !self.payload_transfers_allowed.load(Ordering::SeqCst) {
                return (409, serde_json::to_vec(&json!({"error": "payload transfers are disabled in settings"})).unwrap());
            }
            if rec.paused || rec.checking {
                return (409, serde_json::to_vec(&json!({"error": "payload transfers are paused or saved data is being checked"})).unwrap());
            }
        }
        rec.focused_file = Some(file_index);
        rec.file_only = req.file_only && !rec.direct_play;
        // Go setNextPreloadLocked: warm the next episode only near the end.
        self.set_next_preload(&mut rec, file_index, req.next_file_index);
        if rec.direct_play {
            self.touch_direct_play(&rec.id);
        }
        rec.playback_coordinator.set_play_boost(file_index, req.boost);
        rec.playback_coordinator.set_buffer_experiments(file_index, req.adaptive_buffer, req.burst_aware_buffer);
        if let Some(offset) = req.offset {
            rec.playback_coordinator.prepare_resume(file_index, offset.clamp(0, (file.length - 1).max(0)));
        }
        self.apply_transfer_state(&mut rec);
        drop(rec);
        if let Some(storage) = self.storage_for_torrent(&torrent) {
            storage.set_startup_file(file_index);
        }
        self.notify_change();
        let stream_url = format!(
            "http://127.0.0.1:{}/torrent/{}/file/{}",
            self.stream_port.load(Ordering::SeqCst),
            req.id,
            file_index
        );

        let resp = PlayResponse {
            id: req.id,
            file_index,
            name: file.name.clone(),
            length: file.length,
            stream_url,
        };

        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_file_only(&self, body: &str) -> (u16, Vec<u8>) {
        let req: serde_json::Value = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        let id = match req.get("id").and_then(|v| v.as_str()) {
            Some(i) => i,
            None => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "missing id"})).unwrap(),
                )
            }
        };
        let records = self.records.read();
        if let Some(r) = records.get(id) {
            let enabled = req
                .get("enabled")
                .or_else(|| req.get("fileOnly"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut rec = r.write();
            rec.file_only = enabled && !rec.direct_play;
            self.apply_transfer_state(&mut rec);
            (
                200,
                serde_json::to_vec(&json!({"fileOnly": enabled})).unwrap(),
            )
        } else {
            (
                404,
                serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
            )
        }
    }

    fn handle_focus(&self, body: &str) -> (u16, Vec<u8>) {
        let req: FocusRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        let records = self.records.read();
        if let Some(r) = records.get(&req.id) {
            let mut rec = r.write();
            rec.focused_file = req.file_index;
            self.apply_transfer_state(&mut rec);
            self.notify_change();
            (
                200,
                serde_json::to_vec(&OkResponse {
                    ok: true,
                    id: Some(req.id),
                })
                .unwrap(),
            )
        } else {
            (
                404,
                serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
            )
        }
    }

    fn validate_selection(
        torrent: &wt_core::Torrent,
        selected: &[usize],
        allow_empty: bool,
    ) -> Result<(), String> {
        if !allow_empty && selected.is_empty() {
            return Err("select at least one file".to_string());
        }
        let num_files = torrent.files().len();
        let mut seen = HashSet::new();
        for &index in selected {
            if index >= num_files || !seen.insert(index) {
                return Err(format!("invalid selected file index {index}"));
            }
        }
        Ok(())
    }

    fn handle_configure(&self, body: &str) -> (u16, Vec<u8>) {
        let req: ConfigureRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };

        let (rec_arc, torrent) = match self.record_by_id(&req.id) {
            Some(r) => {
                let rec = r.read();
                (r.clone(), rec.torrent.clone())
            }
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                );
            }
        };

        let selected = req.selected.unwrap_or_default();
        let descriptors = req.descriptors.unwrap_or_default();
        self.close_preview_for(&req.id);

        if let Err(error) = self.wait_for_info(&torrent) {
            return (409, serde_json::to_vec(&json!({"error": error})).unwrap());
        }

        if let Err(msg) = Self::validate_selection(&torrent, &selected, true) {
            return (400, serde_json::to_vec(&json!({"error": msg})).unwrap());
        }

        let Some(storage) = self.storage_for_torrent(&torrent) else {
            return (
                409,
                serde_json::to_vec(&json!({"error": "download storage is not available"})).unwrap(),
            );
        };

        if req.storage_mode.as_deref().is_some_and(|m| m != STORAGE_MODE_STREAM && m != STORAGE_MODE_DOWNLOAD && !m.is_empty()) {
            return (400, serde_json::to_vec(&json!({"error": "invalid storageMode"})).unwrap());
        }
        let stream = req.storage_mode.as_deref() == Some(STORAGE_MODE_STREAM);
        if stream {
            // Go server.go configure: clamp stream RAM and wire onStreamEvict so
            // discarded pieces refresh completion (dirty bits become requestable).
            if let Some(bytes) = req.stream_cache_bytes.filter(|&b| b > 0) {
                storage.set_cache_limit(bytes.clamp(16 << 20, wt_storage::MAX_RAM_CACHE_BYTES));
            }
            let torrent_for_cb = Arc::downgrade(&torrent.inner);
            storage.set_on_stream_evict(Some(Arc::new(move |index| {
                if let Some(inner) = torrent_for_cb.upgrade().filter(|t| !t.closed.load(Ordering::Relaxed)) {
                    wt_core::Torrent { inner }.update_piece_completion(index as u32);
                }
            })));
        }
        if let Err(e) = storage.attach_descriptors_mode(&descriptors, &selected, stream) {
            return (
                400,
                serde_json::to_vec(&json!({"error": e.to_string()})).unwrap(),
            );
        }

        let mut rec = rec_arc.write();
        if torrent.closed() {
            return (409, serde_json::to_vec(&json!({"error": "torrent removed"})).unwrap());
        }
        rec.refresh();
        rec.torrent.update_all_piece_completion();
        rec.configured = true;
        rec.selected = selected;
        rec.direct_play = stream;
        if stream {
            // Match Go: temporary stream-cache pressure must not permanently
            // disable download; the failed chunk is re-requested.
            let t = rec.torrent.clone();
            let t_cb = Arc::downgrade(&t.inner);
            // Direct Play / stream: peer_conn already requeues failed chunks.
            // Only permanent disable for true closure — transient cache/SAF/I/O
            // pressure used to call disallow_data_download and freeze speed at 0
            // while play waited forever for pieces that could no longer arrive.
            t.set_on_write_chunk_error(Some(Arc::new(move |err: &wt_core::CoreError| {
                let fatal = matches!(
                    err,
                    wt_core::CoreError::Storage(wt_types::StorageError::Closed)
                );
                if fatal {
                    if let Some(inner) = t_cb.upgrade() {
                        wt_core::Torrent { inner }.disallow_data_download();
                    }
                }
            })));
        }
        // Go applyPeerHints: reconnect peers that worked last time at once.
        self.apply_peer_hints(&rec);
        let verification = if !stream && storage.needs_verify() {
            let info = torrent.info().expect("configured torrent metadata");
            let total = torrent.files().iter().filter(|f| rec.selected.contains(&f.index()))
                .flat_map(|f| (f.offset() / info.piece_length) as usize..
                    ((f.offset() + f.length() + info.piece_length - 1) / info.piece_length) as usize)
                .collect::<HashSet<_>>().len();
            let progress = Arc::new(wt_storage::VerificationProgress::new(total));
            rec.checking = true;
            rec.checked_pieces = 0;
            rec.check_total = total;
            rec.verification = Some(progress.clone());
            Some((progress, info, rec.selected.clone()))
        } else {
            storage.unfreeze_trusted();
            None
        };
        self.apply_transfer_state(&mut rec);
        drop(rec);
        if let Some((progress, info, selected)) = verification {
            let weak = self.self_ref.get().cloned().unwrap_or_default();
            self.client.inner.tokio_handle.spawn_blocking(move || {
                let result = wt_storage::check_saved_data(storage, &info, &selected, progress.clone());
                let Some(server) = weak.upgrade() else { return; };
                let mut rec = rec_arc.write();
                if rec.torrent.closed() { return; }
                rec.torrent.update_all_piece_completion();
                rec.checked_pieces = progress.checked_pieces.load(Ordering::SeqCst);
                rec.checking = false;
                rec.verification = None;
                if let Err(error) = result { rec.warning = Some(error.to_string()); }
                server.apply_transfer_state(&mut rec);
                drop(rec);
                server.notify_change();
            });
        }
        self.notify_change();
        (
            200,
            serde_json::to_vec(&OkResponse { ok: true, id: None }).unwrap(),
        )
    }

    fn handle_select(&self, body: &str) -> (u16, Vec<u8>) {
        let req: SelectRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };

        let (rec_arc, torrent) = match self.record_by_id(&req.id) {
            Some(r) => {
                let rec = r.read();
                (r.clone(), rec.torrent.clone())
            }
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                );
            }
        };

        let selected_response = req.selected.clone();
        let selected = req.selected.unwrap_or_default();
        if let Err(error) = self.wait_for_info(&torrent) {
            return (409, serde_json::to_vec(&json!({"error": error})).unwrap());
        }

        if let Err(msg) = Self::validate_selection(&torrent, &selected, true) {
            return (400, serde_json::to_vec(&json!({"error": msg})).unwrap());
        }

        let mut rec = rec_arc.write();
        if torrent.closed() {
            return (409, serde_json::to_vec(&json!({"error": "torrent removed"})).unwrap());
        }
        if !rec.configured {
            return (
                409,
                serde_json::to_vec(&json!({"error": "download storage is not configured"}))
                    .unwrap(),
            );
        }

        let Some(storage) = self.storage_for_torrent(&torrent) else {
            return (
                409,
                serde_json::to_vec(&json!({"error": "download storage is not configured"}))
                    .unwrap(),
            );
        };

        if !rec.direct_play {
            if let Some(descriptors) = &req.descriptors {
                if let Err(error) = storage.update_selection(descriptors, &selected) {
                    return (400, serde_json::to_vec(&json!({"error": error.to_string()})).unwrap());
                }
                // Pieces touching a replaced destination lost completion.
                rec.torrent.update_all_piece_completion();
            } else if !storage.selection_can_use_existing_files(&selected) {
                return (400, serde_json::to_vec(&json!({"error": "descriptors are required when selecting a file for the first time"})).unwrap());
            }
        }
        rec.selected = selected;
        self.apply_transfer_state(&mut rec);
        self.notify_change();
        (
            200,
            serde_json::to_vec(&SelectResponse {
                ok: true,
                selected: selected_response,
            })
            .unwrap(),
        )
    }

    fn handle_pause_resume(&self, body: &str, pause: bool) -> (u16, Vec<u8>) {
        let req: IdRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        if let Some(r) = self.record_by_id(&req.id) {
            if pause {
                self.close_preview_for(&req.id);
            }
            let mut rec = r.write();
            rec.paused = pause;
            self.apply_transfer_state(&mut rec);
            self.notify_change();
            (
                200,
                serde_json::to_vec(&OkResponse { ok: true, id: None }).unwrap(),
            )
        } else {
            (
                404,
                serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
            )
        }
    }

    fn handle_remove(&self, body: &str) -> (u16, Vec<u8>) {
        let req: IdRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        self.close_preview_for(&req.id);
        // Detach before closing readers: their callbacks re-enter the host.
        let rec = {
            let mut records = self.records.write();
            let mut by_hash = self.records_by_hash.write();
            records.remove(&req.id).inspect(|rec| {
                by_hash.remove(&rec.read().info_hash);
            })
        };
        self.forget_idle_state(&req.id);
        let Some(rec) = rec else {
            return (404, serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap());
        };
        let (torrent, hash) = {
            let rec = rec.read();
            if let Some(progress) = &rec.verification { progress.cancel(); }
            (rec.torrent.clone(), rec.info_hash.clone())
        };
        torrent.drop_torrent();
        let readers = {
            let mut registry = self.playback_readers.lock();
            let handles: Vec<_> = registry.iter().filter(|(_, r)| r.id == req.id)
                .map(|(&handle, _)| handle).collect();
            handles.into_iter().filter_map(|handle| registry.remove(&handle)).collect::<Vec<_>>()
        };
        for reader in readers { reader.close(); }
        if let Some(storage) = self.storage.get_storage(&hash) {
            storage.set_on_stream_evict(None);
            let _ = wt_types::TorrentStorage::close(storage.as_ref());
        }
        let _ = self.storage.remove_torrent(torrent.info_hash());
        // Go: a missing destroyStore means destroy.
        if req.destroy_store.unwrap_or(true) {
            // A Drop keeps the library entry; its peers reconnect on the next Play.
            self.peer_hints.lock().forget_peer_hints(&self.download_dir, &hash);
            let _ = self.storage.delete_checkpoint(&hash);
            let _ = wt_storage::delete_engine_owned_storage(&self.download_dir, &hash);
            let _ = std::fs::remove_dir_all(wt_storage::resume_window_dir(&self.download_dir, &hash));
        }
        self.tracker_status.clear_status(&hash);
        self.notify_change();
        (200, serde_json::to_vec(&OkResponse { ok: true, id: Some(req.id) }).unwrap())
    }

    fn handle_forget_peers(&self, body: &str) -> (u16, Vec<u8>) {
        let req: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":"invalid json"})).unwrap(),
                )
            }
        };
        if let Some(hash) = req.get("infoHash").and_then(|v| v.as_str()) {
            // Go: forgets hints even after the live torrent was dropped.
            self.peer_hints.lock().forget_peer_hints(&self.download_dir, hash);
            if let Some(r) = self.records_by_hash.read().get(hash) {
                r.read().torrent.forget_peers();
            }
        }
        (200, serde_json::to_vec(&json!({"ok":true})).unwrap())
    }

    fn handle_pieces(&self, id: &str, query: &str) -> (u16, Vec<u8>) {
        self.piece_telemetry(id, query)
    }

    fn handle_preview_state(&self, id: &str, query: &str) -> (u16, Vec<u8>) {
        let records = self.records.read();
        let Some(r) = records.get(id) else {
            return (
                404,
                serde_json::to_vec(&json!({"error":"torrent not found"})).unwrap(),
            );
        };
        let rec = r.read();
        let index = url::form_urlencoded::parse(query.as_bytes()).find_map(|(k, v)| {
            if k == "fileIndex" {
                v.parse::<usize>().ok()
            } else {
                None
            }
        });
        let Some(file) = index.and_then(|i| rec.torrent.files().get(i).cloned()) else {
            return (
                400,
                serde_json::to_vec(&json!({"error":"invalid file index"})).unwrap(),
            );
        };
        let pieces = crate::live_preview::pieces_overlapping_file(
            (0..rec.torrent.num_pieces()).map(|p| (p, rec.torrent.piece_complete(p))),
            rec.piece_length,
            file.offset(),
            file.length(),
        );
        let response =
            crate::live_preview::build_preview_state(crate::live_preview::PreviewStateInput {
                active: self.playback_readers.lock().values().any(|reader| {
                    reader.id == id && reader.file_index == file.index() && !reader.is_closed()
                }),
                byte_offset: rec.playback_coordinator.preview_position(file.index()),
                completed_piece_indices: pieces,
            });
        (200, serde_json::to_vec(&response).unwrap())
    }

    fn handle_connections(&self, id: &str) -> (u16, Vec<u8>) {
        let records = self.records.read();
        let rec = match records.get(id) {
            Some(r) => r.read(),
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                )
            }
        };

        let mut peers = Vec::new();
        if !rec.paused {
            for peer in rec.torrent.peer_conns() {
                let addr = if peer.remote_addr.ip().is_unspecified() {
                    std::net::SocketAddr::new(
                        match peer.remote_addr.ip() {
                            std::net::IpAddr::V4(_) => {
                                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                            }
                            std::net::IpAddr::V6(_) => {
                                std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
                            }
                        },
                        peer.remote_addr.port(),
                    )
                } else {
                    peer.remote_addr
                };
                let address = addr.to_string();
                let country = self.geoip.lookup_country(&address);
                let transport = match peer.transport {
                    wt_types::Transport::Tcp => "TCP".to_string(),
                    wt_types::Transport::Utp => "uTP".to_string(),
                    wt_types::Transport::WebRtc => "WebRTC".to_string(),
                    wt_types::Transport::WebSeed => "WebSeed".to_string(),
                };
                peers.push(PeerConnectionView {
                    address,
                    transport,
                    client: peer.client_name,
                    downloaded: peer.conn_stats.bytes_read_data,
                    uploaded: peer.conn_stats.bytes_written_data,
                    country,
                    peer_choking: Some(peer.peer_choking),
                    am_interested: Some(peer.am_interested),
                    pending_requests: Some(peer.pending_requests),
                    peer_completion: Some(peer.peer_completion),
                    download_rate: Some(peer.download_rate),
                    rtt_ms: Some(peer.rtt.as_secs_f64() * 1000.0),
                    pipeline_limit: Some(peer.pipeline_limit),
                    nominal_max_requests: Some(peer.nominal_max_requests),
                    peer_max_requests: Some(peer.peer_max_requests),
                    peak_requests: Some(peer.peak_requests),
                });
            }
        }
        sort_peer_connections(&mut peers);

        let resp = ConnectionsResponse {
            id: rec.id.clone(),
            paused: rec.paused,
            peers,
            diagnostics: std::env::var_os("WT_SWARM_DIAGNOSTICS").map(|_| rec.torrent.swarm_diagnostics()),
        };
        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_metadata(&self, id: &str) -> (u16, Vec<u8>) {
        let records = self.records.read();
        let rec = match records.get(id) {
            Some(r) => r.read(),
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                )
            }
        };

        if let Ok(mut mi) = rec.torrent.metainfo() {
            mi.announce.clear();
            mi.announce_list = rec.original_trackers.clone();
            mi.comment = "dynamic metainfo from client".into();
            mi.created_by = "https://github.com/anacrolix/torrent".into();
            mi.creation_date = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if let Ok(mut data) = wt_meta::encode_metainfo(&mi) {
                // Go's generated metainfo includes empty piece-layers and URL-list containers.
                if mi.piece_layers.is_empty()
                    && mi.url_list.is_empty()
                    && data.last() == Some(&b'e')
                {
                    data.pop();
                    data.extend_from_slice(b"12:piece layersde8:url-listlee");
                }
                let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
                return (
                    200,
                    serde_json::to_vec(&MetadataResponse { torrent_data: b64 }).unwrap(),
                );
            }
        }
        (
            404,
            serde_json::to_vec(&json!({"error":"metadata not available"})).unwrap(),
        )
    }

    fn handle_get_settings(&self) -> (u16, Vec<u8>) {
        let resp = SettingsResponse {
            ghost_mode: self.ghost_mode.load(Ordering::SeqCst),
            ghost_pro_mode: self.ghost_pro_mode.load(Ordering::SeqCst),
            dial_rate_per_sec: self.dial_rate_per_sec.load(Ordering::SeqCst),
            effective_dial_rate_per_sec: self.client.inner.dial_limiter.limit() as i64,
            peer_exploration: self.peer_exploration.load(Ordering::SeqCst),
            claude_seek_mode: self.claude_seek_mode.load(Ordering::SeqCst),
            hedge_requests: self.hedge_requests.load(Ordering::SeqCst),
            slow_peer_yield: self.slow_peer_yield.load(Ordering::SeqCst),
            drop_chokers: self.drop_chokers.load(Ordering::SeqCst),
            deadline_seek: self.deadline_seek.load(Ordering::SeqCst),
            idle_drop_minutes: self.idle_drop_minutes.load(Ordering::SeqCst),
            ram_cache_bytes: self.ram_cache_bytes.load(Ordering::SeqCst),
            seeding_enabled: self.seeding_enabled.load(Ordering::SeqCst),
            seed_ratio_milli: self.seed_ratio_milli.load(Ordering::SeqCst),
            seed_time_minutes: self.seed_time_minutes.load(Ordering::SeqCst),
            max_peers: self.max_peers.load(Ordering::SeqCst),
            download_rate_bytes_sec: self.download_rate_limit.load(Ordering::SeqCst),
            upload_rate_bytes_sec: self.upload_rate_limit.load(Ordering::SeqCst),
            payload_transfers_allowed: self.payload_transfers_allowed.load(Ordering::SeqCst),
            seek_stats: {
                let snap = self.seek_stats.lock().snapshot();
                SeekStats {
                    count: snap.count.max(0) as usize,
                    last_ms: snap.last_ms,
                    median_ms: snap.median_ms,
                    stalls: snap.stalls.max(0) as usize,
                }
            },
        };
        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_post_settings(&self, body: &str) -> (u16, Vec<u8>) {
        let req: SettingsRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };

        // Validate the complete request before changing connection limits or policy.
        if let Some(cache_bytes) = req.ram_cache_bytes {
            if !(16 * 1024 * 1024..=1024 * 1024 * 1024).contains(&cache_bytes) {
                return (
                    400,
                    serde_json::to_vec(
                        &json!({"error": "RAM cache must be between 16 and 1024 MiB"}),
                    )
                    .unwrap(),
                );
            }
        }
        if let Some(peers) = req.max_peers.filter(|&p| p != 0) {
            if !(8..=500).contains(&peers) {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "maxPeers must be between 8 and 500"}))
                        .unwrap(),
                );
            }
            self.max_peers.store(peers, Ordering::SeqCst);
            let (half_open_per, half_open_total) = wt_core::half_open_for_max_peers(peers);
            self.client.set_half_open_limits(half_open_per, half_open_total);
            let records: Vec<_> = self.records.read().values().cloned().collect();
            for record in records {
                let rec = record.read();
                if !rec.resting {
                    rec.torrent.set_max_established_conns(peers);
                }
            }
        }

        if let Some(cache_bytes) = req.ram_cache_bytes {
            self.ram_cache_bytes.store(cache_bytes, Ordering::SeqCst);
            self.storage.set_memory_limit(cache_bytes);
        }

        if let Some(dial) = req.dial_rate_per_sec {
            let clamped = dial.clamp(10, 80) as usize;
            self.dial_rate_per_sec.store(clamped, Ordering::SeqCst);
            self.client.inner.dial_limiter.set_limit(clamped as f64);
        }

        if let Some(pro) = req.ghost_pro_mode {
            self.ghost_pro_mode.store(pro, Ordering::SeqCst);
            if pro {
                self.ghost_mode.store(false, Ordering::SeqCst);
                self.peer_exploration.store(false, Ordering::SeqCst);
            }
        }
        if let Some(ghost) = req.ghost_mode {
            self.ghost_mode.store(ghost, Ordering::SeqCst);
            if ghost {
                self.ghost_pro_mode.store(false, Ordering::SeqCst);
            }
        }
        if let Some(exp) = req.peer_exploration {
            self.peer_exploration.store(exp, Ordering::SeqCst);
            if exp {
                self.ghost_pro_mode.store(false, Ordering::SeqCst);
            }
        }
        // Go: Ghost Pro widens video startup hints; reapply them.
        if req.ghost_pro_mode.is_some() || req.ghost_mode.is_some() || req.peer_exploration.is_some() {
            let records: Vec<_> = self.records.read().values().cloned().collect();
            for rec in records {
                self.refresh_startup_hints(&mut rec.write());
            }
        }
        if let Some(seek) = req.claude_seek_mode {
            let prev = self.claude_seek_mode.swap(seek, Ordering::SeqCst);
            if prev != seek {
                let after = if seek {
                    std::time::Duration::from_millis(500)
                } else {
                    std::time::Duration::ZERO
                };
                self.client.set_urgent_request_stealing(after);
                self.seek_stats.lock().reset();
            }
        }
        let mut retune = false;
        if let Some(h) = req.hedge_requests {
            if self.hedge_requests.swap(h, Ordering::SeqCst) != h {
                retune = true;
            }
        }
        if let Some(s) = req.slow_peer_yield {
            if self.slow_peer_yield.swap(s, Ordering::SeqCst) != s {
                retune = true;
            }
        }
        if let Some(d) = req.drop_chokers {
            self.drop_chokers.store(d, Ordering::SeqCst);
        }
        if let Some(deadline) = req.deadline_seek {
            if self.deadline_seek.swap(deadline, Ordering::SeqCst) != deadline {
                retune = true;
                // Seek times restart so the two methods can be compared.
                self.seek_stats.lock().reset();
            }
        }
        if retune
            || req.hedge_requests.is_some()
            || req.slow_peer_yield.is_some()
        {
            self.apply_seek_tuning();
        }
        if let Some(idle) = req.idle_drop_minutes {
            self.idle_drop_minutes
                .store(idle.clamp(0, 240), Ordering::SeqCst);
        }
        if let Some(se) = req.seeding_enabled {
            self.seeding_enabled.store(se, Ordering::SeqCst);
        }
        if let Some(sa) = req.seeding_allowed {
            self.seeding_allowed.store(sa, Ordering::SeqCst);
        }
        if let Some(ratio) = req.seed_ratio_milli.filter(|v| (100..=10000).contains(v)) {
            self.seed_ratio_milli.store(ratio, Ordering::SeqCst);
        }
        if let Some(mins) = req.seed_time_minutes.filter(|v| (1..=1440).contains(v)) {
            self.seed_time_minutes.store(mins, Ordering::SeqCst);
        }
        if let Some(down) = req.download_rate_bytes_sec.filter(|&rate| rate >= 0) {
            self.download_rate_limit.store(down, Ordering::SeqCst);
            Self::set_transfer_limiter(&self.client.inner.download_limiter, down);
        }
        if let Some(up) = req.upload_rate_bytes_sec.filter(|&rate| rate >= 0) {
            self.upload_rate_limit.store(up, Ordering::SeqCst);
            Self::set_transfer_limiter(&self.client.inner.upload_limiter, up);
        }
        if let Some(allowed) = req.payload_transfers_allowed {
            self.payload_transfers_allowed
                .store(allowed, Ordering::SeqCst);
        }

        // File priorities and upload admission are independent of dialing/RAM settings.
        if req.payload_transfers_allowed.is_some()
            || req.seeding_enabled.is_some()
            || req.seeding_allowed.is_some()
            || req.seed_ratio_milli.is_some()
            || req.seed_time_minutes.is_some()
        {
            let records: Vec<_> = self.records.read().values().cloned().collect();
            for record in records {
                self.apply_transfer_state(&mut record.write());
            }
        }
        if req.ghost_mode.is_some()
            || req.ghost_pro_mode.is_some()
            || req.peer_exploration.is_some()
            || req.dial_rate_per_sec.is_some()
            || req.max_peers.is_some_and(|peers| peers != 0)
            || req.download_rate_bytes_sec.is_some()
            || req.payload_transfers_allowed.is_some()
        {
            self.reset_ghost();
        }
        if req.ghost_mode.is_some() || req.ghost_pro_mode.is_some()
            || req.peer_exploration.is_some() || req.ram_cache_bytes.is_some()
        {
            let ids: Vec<_> = self.records.read().keys().cloned().collect();
            for id in ids { self.refresh_playback_windows(&id); }
        }
        self.notify_change();
        self.handle_get_settings()
    }

    fn set_transfer_limiter(limiter: &wt_core::ratelimit::Limiter, rate: i64) {
        if rate == 0 {
            limiter.set_limit(f64::INFINITY);
        } else {
            limiter.set_limit(rate as f64);
            limiter.set_burst(rate.clamp(1, 1024 * 1024) as usize);
        }
    }

    fn handle_network_change(&self, body: &str) -> (u16, Vec<u8>) {
        let (generation, connected) = match NetworkManager::parse_change_body(body) {
            Ok(v) => v,
            Err(e) => return (400, serde_json::to_vec(&json!({"error":e})).unwrap()),
        };
        // Duplicate or delayed callbacks are ignored: a policy update must not
        // tear down healthy peers on the same network.
        if let Some(connected) = self.network.on_network_change(generation, connected) {
            self.reset_ghost();
            if connected {
                if let Some(server) = self.self_ref.get().and_then(|w| w.upgrade()) {
                    let episode = self.network.episode();
                    std::thread::Builder::new()
                        .name("wt-net-recover".into())
                        .spawn(move || server.recover_network(episode))
                        .ok();
                }
            }
        }
        self.notify_change();
        (
            200,
            serde_json::to_vec(&OkResponse { ok: true, id: None }).unwrap(),
        )
    }

    fn handle_get_bypass(&self) -> (u16, Vec<u8>) {
        let enabled = self.bypass_enabled.load(Ordering::SeqCst);
        // Nothing wraps peer or tracker sockets in this engine yet, so the
        // switch is remembered and reported as not running.
        let status = BypassStatus {
            enabled,
            active: false,
            mode: wt_policy::BYPASS_MODE_OFF.to_string(),
            coverage: None,
            error: enabled.then(|| "Anti-DPI is not available in this version".to_string()),
        };
        (200, serde_json::to_vec(&status).unwrap())
    }

    fn handle_post_bypass(&self, body: &str) -> (u16, Vec<u8>) {
        let req: serde_json::Value = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        let enabled = req
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.bypass_enabled.store(enabled, Ordering::SeqCst);
        self.handle_get_bypass()
    }

    /// Go `cacheStatus`: on-disk size of each cache, and how much of it a
    /// clear would free now. Caches of sessions in use are kept by a clear.
    fn handle_get_cache(&self) -> (u16, Vec<u8>) {
        use wt_storage::cache_store::{dht_contacts_path, dir_bytes, peer_hints_path};
        let base = &self.download_dir;
        let file_bytes = |path: PathBuf| std::fs::metadata(path).map_or(0, |m| m.len() as i64);
        let hashes: Vec<String> = self.records.read().values()
            .map(|r| r.read().info_hash.clone()).collect();
        let mut memory = 0i64;
        let mut startup_kept = 0i64;
        for hash in &hashes {
            if let Some(storage) = self.storage.get_storage(hash) {
                memory += storage.cache_bytes();
                if storage.stream_mode() {
                    startup_kept += dir_bytes(&base.join(".startup").join(hash));
                }
            }
        }
        let resume_kept: i64 = self.protected_resume_hashes().iter()
            .map(|hash| dir_bytes(&wt_storage::resume_window_dir(base, hash))).sum();
        let startup = dir_bytes(&base.join(".startup"));
        let resume = dir_bytes(&base.join(".cache").join("resume"));
        let peers = file_bytes(peer_hints_path(base));
        let dht = file_bytes(dht_contacts_path(base));
        let category = |id: &str, bytes: i64, reclaimable: i64| CacheCategoryView {
            id: id.to_string(),
            bytes,
            reclaimable_bytes: reclaimable.clamp(0, bytes.max(0)),
        };
        let resp = CacheStatusResponse {
            categories: vec![
                category("startup", startup, startup - startup_kept),
                category("resume", resume, resume - resume_kept),
                category("peers", peers, peers),
                category("dht", dht, dht),
            ],
            memory_bytes: memory,
        };
        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_clear_cache(&self, body: &str) -> (u16, Vec<u8>) {
        let value: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":"invalid json"})).unwrap(),
                )
            }
        };
        let categories: Vec<String> = value
            .get("categories")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        match self.clear_engine_cache(&categories) {
            Ok(()) => self.handle_get_cache(),
            Err(e) => (
                400,
                serde_json::to_vec(&json!({"error":e.to_string()})).unwrap(),
            ),
        }
    }

    /// Public control entry point for integration tests and JNI layer.
    ///
    /// Takes `method` ("GET", "POST"), `path` ("/stats", "/torrent/...", etc.),
    /// and `body` (JSON string or empty), and returns `(http_status_code, response_body_bytes)`.
    pub fn request(&self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        self.dispatch_control(method, path, body)
    }

    pub fn open_playback(&self, id: &str, index: usize) -> std::io::Result<Arc<PlaybackReader>> {
        self.open_playback_at(id, index, -1)
    }

    /// Open at the player's Range start when it has one past the file's start.
    /// Opening at the last played position instead sent every peer to the old
    /// spot, and held the real refocus back behind its debounce.
    pub fn open_playback_at(&self, id: &str, index: usize, requested: i64) -> std::io::Result<Arc<PlaybackReader>> {
        let (rec_arc, torrent) = self
            .record_by_id(id)
            .map(|r| {
                let rec = r.read();
                (r.clone(), rec.torrent.clone())
            })
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "torrent not found")
            })?;
        self.wait_for_info(&torrent)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::WouldBlock, e))?;
        // Play must not inherit a stuck disallow_data_download from a prior
        // Direct Play write glitch — re-apply transfer gates so peers resume.
        {
            let mut rec = rec_arc.write();
            self.apply_transfer_state(&mut rec);
        }
        let coordinator = rec_arc.read().playback_coordinator.clone();
        if let Some(offset) = self.apply_resume_window_to_storage(id, index) {
            coordinator.remember_resume(index, offset);
        }
        let start = if requested > 0 { requested } else { coordinator.preview_position(index) };
        let host = self
            .self_ref
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| std::io::Error::other("engine closed"))?;
        let handle = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        let (_, reader) = PlaybackReader::open_for_host(host, coordinator, id, index, start, handle)?;
        let mut readers = self.playback_readers.lock();
        readers.retain(|_, reader| !reader.is_closed());
        readers.insert(handle, reader.clone());
        drop(readers);
        self.refresh_playback_windows(id);
        Ok(reader)
    }

    fn refresh_playback_windows(&self, id: &str) {
        let readers: Vec<_> = self.playback_readers.lock().values()
            .filter(|reader| reader.id == id && !reader.is_closed()).cloned().collect();
        for reader in readers { reader.refresh_window(); }
    }

    pub fn open_preview(&self, id: &str, index: usize) -> std::io::Result<Arc<PreviewReader>> {
        let reader = crate::preview::open_preview(self, id, index)?;
        *self.preview_active_id.lock() = Some(id.to_string());
        Ok(reader)
    }
}

impl crate::playback::PlaybackHost for EngineServer {
    fn record_seek(&self, latency: Duration) {
        self.seek_stats.lock().record_seek(latency);
    }
    fn begin_playback_read(&self, id: &str) {
        EngineServer::begin_playback_read(self, id);
    }
    fn end_playback_read(&self, id: &str) {
        EngineServer::end_playback_read(self, id);
    }
    fn playback_near_end(&self, id: &str, file_index: usize) {
        // Takes the record lock; the reader holds its session lock here.
        let Some(server) = self.self_ref.get().and_then(|w| w.upgrade()) else { return; };
        let id = id.to_string();
        std::thread::spawn(move || server.start_next_preload(&id, file_index));
    }

    fn torrent_for_playback(&self, id: &str) -> Option<crate::playback::PlaybackTorrentSnapshot> {
        let records = self.records.read();
        let rec = records.get(id)?.read();
        let info = rec.torrent.info()?;
        let len = info.piece_length;
        if len <= 0 {
            return None;
        }
        let files = rec
            .torrent
            .files()
            .into_iter()
            .map(|f| crate::playback::PlaybackFileInfo {
                index: f.index(),
                name: f.path(),
                length: f.length(),
                torrent_offset: f.offset(),
                begin_piece: (f.offset() / len) as u32,
                end_piece: ((f.offset() + f.length() + len - 1) / len) as u32,
                verified: f.bytes_completed() == f.length(),
            })
            .collect();
        Some(crate::playback::PlaybackTorrentSnapshot {
            configured: rec.configured,
            direct_play: rec.direct_play,
            file_only: rec.file_only,
            selected: rec.selected.clone(),
            focused_file: rec.focused_file,
            files,
            piece_length: len,
            open_reader_count: self.playback_readers.lock().values()
                .filter(|reader| reader.id == id && !reader.is_closed()).count(),
        })
    }
    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }
    fn on_playback_closed(&self, id: &str, file_index: usize) {
        if let Some(record) = self.record_by_id(id) {
            let rec = record.read();
            let coordinator = rec.playback_coordinator.clone();
            let offset = coordinator.preview_position(file_index);
            if offset <= 0 { return }
            let has_other_reader = self.playback_readers.lock().values().any(|reader| {
                reader.id == id && reader.file_index == file_index && !reader.is_closed()
            });
            if !has_other_reader && !rec.paused && !self.shutting_down.load(Ordering::SeqCst)
                && self.payload_transfers_allowed.load(Ordering::SeqCst)
            {
                if let Some(file) = rec.torrent.files().get(file_index) {
                    let piece_length = rec.piece_length;
                    if piece_length > 0 {
                        let begin = (file.offset() + offset) / piece_length;
                        let end = (file.offset() + (offset + wt_policy::constants::PLAYBACK_URGENT_BYTES).min(file.length()) + piece_length - 1) / piece_length;
                        let generation = coordinator.hold_resume((begin..end).map(|piece| (piece as u32, wt_types::PiecePriority::High)).collect());
                        let weak = self.self_ref.get().cloned().unwrap_or_default();
                        let id = id.to_owned();
                        self.client.inner.tokio_handle.spawn(async move {
                            tokio::time::sleep(Duration::from_secs(90)).await;
                            if let (Some(server), Some(demand)) = (weak.upgrade(), coordinator.expire_resume(generation)) {
                                server.publish_demand(&id, &demand);
                            }
                        });
                    }
                }
            }
            let effective = rec.playback_coordinator.effective();
            drop(rec);
            self.publish_demand(id, &effective);
            self.schedule_resume_persist(id, file_index, offset);
        }
    }

    fn playback_readers_changed(&self, id: &str) {
        if !self.shutting_down.load(Ordering::SeqCst) { self.refresh_playback_windows(id); }
    }
    fn payload_allowed(&self) -> bool {
        self.payload_transfers_allowed.load(Ordering::SeqCst)
    }
    fn playback_allowed(&self, id: &str, index: usize) -> bool {
        self.record_by_id(id).is_some_and(|record| {
            let rec = record.read();
            if rec.torrent.closed() || !rec.configured { return false; }
            let file = rec.torrent.files().get(index).cloned();
            file.is_some_and(|file| file.bytes_completed() >= file.length()
                || (!rec.paused && !rec.checking && rec.error.is_none()
                    && self.payload_transfers_allowed.load(Ordering::SeqCst)))
        })
    }
    fn ghost_pro(&self) -> bool {
        self.ghost_pro_mode.load(Ordering::SeqCst)
    }
    fn open_file_reader(
        &self,
        id: &str,
        index: usize,
    ) -> std::io::Result<Box<dyn crate::playback::ReaderBackend>> {
        let records = self.records.read();
        let file = records
            .get(id)
            .and_then(|r| r.read().torrent.files().get(index).cloned())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "torrent or file not found")
            })?;
        let mut backend = if let Some(storage) = self.storage.get_storage(&file.torrent.info_hash.to_hex()) {
            crate::playback_core::CoreReaderBackend::from_file_with_storage(&file, storage)
        } else {
            crate::playback_core::CoreReaderBackend::from_file(&file)
        };
        let server = self.self_ref.get().cloned().unwrap_or_default();
        backend.set_on_stall(Arc::new(move || {
            if let Some(server) = server.upgrade() {
                server.seek_stats.lock().record_stall();
                wt_types::events::record("stall", "");
            }
        }));
        Ok(Box::new(backend))
    }
    fn apply_piece_priorities(&self, id: &str, demand: &HashMap<u32, wt_core::PiecePriority>) {
        if let Some(record) = self.record_by_id(id) {
            Self::apply_record_piece_priorities(&mut record.write(), demand);
        }
    }
    fn clear_piece_priorities(&self, id: &str, pieces: &[u32]) {
        if let Some(r) = self.records.read().get(id) {
            let mut rec = r.write();
            for &piece in pieces {
                rec.playback_demand.remove(&piece);
                rec.torrent
                    .set_piece_priority(piece, wt_core::PiecePriority::None);
            }
        }
    }
    fn set_request_focus(&self, id: &str, offset: i64) {
        if let Some(r) = self.records.read().get(id) {
            r.read().torrent.set_request_focus(offset);
        }
    }
    fn set_playback_interest(&self, id: &str, begin: u32, end: u32) {
        if let Some(r) = self.records.read().get(id) {
            r.read().torrent.set_playback_interest(begin, end);
        }
    }
    fn refocus_requests(&self, id: &str) {
        if let Some(r) = self.records.read().get(id) {
            r.read().torrent.refocus_requests_debounced();
        }
    }
    fn nudge_urgent(&self, id: &str) -> usize {
        self.records
            .read()
            .get(id)
            .map_or(0, |r| r.read().torrent.nudge_urgent_requests())
    }
    fn on_focus_changed(&self, id: &str, index: usize) {
        if let Some(r) = self.records.read().get(id) {
            r.write().focused_file = Some(index);
        }
    }
    fn piece_complete(&self, id: &str, piece: u32) -> bool {
        self.records
            .read()
            .get(id)
            .is_some_and(|r| r.read().torrent.piece_complete(piece))
    }

    fn playback_cache_budget(&self, id: &str) -> Option<i64> {
        let rec = self.record_by_id(id)?;
        let rec = rec.read();
        if !rec.direct_play {
            return None;
        }
        self.storage.get_storage(&rec.info_hash).map(|st| st.max_cache_bytes())
    }

    fn read_verified(&self, id: &str, abs: i64, length: i64) -> Option<Vec<u8>> {
        let rec = self.record_by_id(id)?;
        let hash = rec.read().info_hash.clone();
        self.storage.get_storage(&hash)?.copy_verified_range(abs, length)
    }

    fn refresh_playback_completion(&self, id: &str, pieces: &[u32]) {
        if let Some(rec) = self.record_by_id(id) {
            let torrent = rec.read().torrent.clone();
            for &piece in pieces {
                torrent.update_piece_completion(piece);
            }
        }
    }

    fn set_playback_storage_demand(&self, id: &str, demand: &HashMap<u32, wt_core::PiecePriority>) {
        if let Some(rec) = self.record_by_id(id) {
            let hash = rec.read().info_hash.clone();
            if let Some(st) = self.storage.get_storage(&hash) {
                st.set_stream_demand(demand.iter()
                    .filter(|(_, priority)| **priority != wt_core::PiecePriority::None)
                    .map(|(&piece, _)| (piece as usize, true)).collect());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_stats_and_version() {
        let (_dir, server) = settings_tests::test_server();
        let (code, body) = server.request("GET", "/stats", "");
        assert_eq!(code, 200);
        let stats: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(stats["torrents"], 0);

        let (code, body) = server.request("GET", "/version", "");
        assert_eq!(code, 200);
        let ver: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert!(ver.get("version").is_some());
    }

    #[test]
    fn test_server_torrent_lifecycle() {
        let (_dir, server) = settings_tests::test_server();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let add_payload = serde_json::json!({
            "torrentId": hash,
        })
        .to_string();

        let (code, body) = server.request("POST", "/add", &add_payload);
        assert_eq!(code, 200);
        let record: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        let tid = record["id"].as_str().expect("id string");

        // Get torrent
        let (code, body) = server.request("GET", &format!("/torrent/{tid}"), "");
        assert_eq!(code, 200);
        let view: serde_json::Value = serde_json::from_slice(&body).expect("valid view");
        assert_eq!(view["id"], tid);

        // Pause
        let pause_payload = serde_json::json!({
            "id": tid,
        })
        .to_string();
        let (code, _) = server.request("POST", "/pause", &pause_payload);
        assert_eq!(code, 200);

        // Resume
        let resume_payload = serde_json::json!({
            "id": tid,
        })
        .to_string();
        let (code, _) = server.request("POST", "/resume", &resume_payload);
        assert_eq!(code, 200);

        // Remove
        let remove_payload = serde_json::json!({
            "id": tid,
        })
        .to_string();
        let (code, _) = server.request("POST", "/remove", &remove_payload);
        assert_eq!(code, 200);

        let (code, _) = server.request("GET", &format!("/torrent/{tid}"), "");
        assert_eq!(code, 404);
    }

    #[test]
    fn test_server_configure_and_settings() {
        let (_dir, server) = settings_tests::test_server();
        let cfg_payload = serde_json::json!({
            "maxPeers": 88,
            "ghostMode": true,
        })
        .to_string();

        let (code, _) = server.request("POST", "/settings", &cfg_payload);
        assert_eq!(code, 200);

        let (code, body) = server.request("GET", "/settings", "");
        assert_eq!(code, 200);
        let settings: serde_json::Value = serde_json::from_slice(&body).expect("valid json");
        assert_eq!(settings["maxPeers"], 88);
        assert_eq!(settings["ghostMode"], true);
    }
}

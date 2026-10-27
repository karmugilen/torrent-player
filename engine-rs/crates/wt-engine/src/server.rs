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
use crate::playback::{start_stream_server, PlaybackReader, StreamServer};
use crate::preview::PreviewReader;
use crate::seeding::{SeedAccount, SeedingManager};
use crate::tracker_status::{build_tracker_views, TrackerStatusManager};
use crate::trackers::TrackerPool;
use crate::types::*;

pub struct TorrentRecord {
    pub torrent: wt_core::Torrent,
    playback_coordinator: Arc<crate::playback::PlaybackPriorityCoordinator>,
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
    pub verified_bytes: i64,
    pub piece_verified: Vec<bool>,
    pub next_preload: Option<NextPreloadView>,
    pub playback_readers: usize,
    pub resting: bool,
    pub idle_drop_ready: bool,
}

impl TorrentRecord {
    fn refresh(&mut self) {
        let Some(info) = self.torrent.info() else {
            return;
        };
        self.name = Some(info.name.clone());
        self.piece_length = info.piece_length;
        self.total_pieces = self.torrent.num_pieces() as usize;
        self.last_piece_length = if self.total_pieces == 0 {
            0
        } else {
            info.piece((self.total_pieces - 1) as u32).length
        };
        self.piece_verified = (0..self.total_pieces)
            .map(|i| self.torrent.piece_complete(i as u32))
            .collect();
        self.verified_bytes = self.torrent.bytes_completed();
        self.files = self
            .torrent
            .files()
            .into_iter()
            .map(|file| {
                let path = file.path();
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                let verified_bytes = file.bytes_completed();
                let length = file.length();
                FileView {
                    index: file.index(),
                    file_type: crate::stream_http::content_type_for_name(&name).to_string(),
                    name,
                    path,
                    length,
                    progress: if length > 0 {
                        verified_bytes as f64 / length as f64
                    } else {
                        0.0
                    },
                    verified_bytes,
                }
            })
            .collect();
    }
}

pub struct EngineServer {
    self_ref: OnceLock<Weak<EngineServer>>,
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
    pub ghost_mode: AtomicBool,
    pub ghost_pro_mode: AtomicBool,
    pub dial_rate_per_sec: AtomicUsize,
    pub peer_exploration: AtomicBool,
    pub claude_seek_mode: AtomicBool,
    pub seek_stats: Mutex<SeekStats>,
    pub hedge_requests: AtomicBool,
    pub slow_peer_yield: AtomicBool,
    pub drop_chokers: AtomicBool,
    pub learned_arrival: AtomicBool,
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
    pub preview_readers: Mutex<HashMap<i64, Arc<PreviewReader>>>,
    pub playback_readers: Mutex<HashMap<i64, Arc<PlaybackReader>>>,
    pub next_reader_id: AtomicI64,
    pub bypass_enabled: AtomicBool,
}

impl EngineServer {
    pub fn new(
        ctl_port: u16,
        download_dir: impl AsRef<Path>,
        max_peers: usize,
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
                std::thread::spawn(move || {
                    hints
                        .lock()
                        .remember_peer_hint(&dir, &hash.to_hex(), &peer, false)
                });
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
        let config = wt_core::ClientConfig {
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
            half_open_conns_per_torrent: 24,
            total_half_open_conns: 48,
            torrent_peers_high_water: 120.max(effective_peers * 4),
            torrent_peers_low_water: 40.max(effective_peers * 2),
            http_user_agent: format!("TorrentPlayer/{}", crate::version::app_version()),
            extended_handshake_client_version: format!(
                "Torrent Player {} (Go)",
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
            ghost_mode: AtomicBool::new(false),
            ghost_pro_mode: AtomicBool::new(false),
            dial_rate_per_sec: AtomicUsize::new(30),
            peer_exploration: AtomicBool::new(false),
            claude_seek_mode: AtomicBool::new(false),
            seek_stats: Mutex::new(SeekStats::default()),
            hedge_requests: AtomicBool::new(true),
            slow_peer_yield: AtomicBool::new(true),
            drop_chokers: AtomicBool::new(true),
            learned_arrival: AtomicBool::new(true),
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
            preview_readers: Mutex::new(HashMap::new()),
            playback_readers: Mutex::new(HashMap::new()),
            next_reader_id: AtomicI64::new(1),
            bypass_enabled: AtomicBool::new(false),
        });
        let _ = server.self_ref.set(Arc::downgrade(&server));
        Ok(server)
    }

    fn apply_seed_policy(&self, rec: &mut TorrentRecord) {
        rec.seeding = false;
        if rec.paused || !self.payload_transfers_allowed.load(Ordering::SeqCst) {
            rec.torrent.disallow_data_upload();
            return;
        }
        if !rec.configured || rec.direct_play || rec.selected.is_empty() {
            return;
        }
        let files = rec.torrent.files();
        let selected: Vec<_> = files
            .iter()
            .filter(|f| rec.selected.contains(&f.index()))
            .collect();
        let size: i64 = selected.iter().map(|f| f.length()).sum();
        let complete = selected.len() == rec.selected.len()
            && size > 0
            && selected.iter().all(|f| f.bytes_completed() >= f.length());
        if !complete {
            rec.torrent.allow_data_upload();
            return;
        }
        let account = rec.seed_base;
        let uploaded = account.uploaded + rec.torrent.stats().conn_stats.bytes_written_data;
        let ratio = uploaded as f64 / size.max(account.downloaded).max(1) as f64;
        let allowed = self.seeding_enabled.load(Ordering::SeqCst)
            && self.seeding_allowed.load(Ordering::SeqCst)
            && ratio < self.seed_ratio_milli.load(Ordering::SeqCst) as f64 / 1000.0
            && account.seconds < self.seed_time_minutes.load(Ordering::SeqCst) * 60;
        if allowed {
            rec.torrent.allow_data_upload();
            rec.seeding = true;
        } else {
            rec.torrent.disallow_data_upload();
        }
    }

    fn apply_transfer_state(&self, rec: &mut TorrentRecord) {
        let active = self.payload_transfers_allowed.load(Ordering::SeqCst)
            && rec.configured
            && !rec.paused
            && rec.error.is_none();
        if !active {
            rec.torrent.disallow_data_download();
            rec.torrent.disallow_data_upload();
        }
        if rec.torrent.info().is_some() {
            let files = rec.torrent.files();
            let only = rec.file_only && rec.focused_file.is_some() && !rec.direct_play;
            for file in files {
                let priority = if active && rec.selected.contains(&file.index()) && !rec.direct_play
                {
                    if only {
                        if Some(file.index()) == rec.focused_file {
                            wt_core::PiecePriority::Normal
                        } else {
                            wt_core::PiecePriority::None
                        }
                    } else {
                        wt_core::PiecePriority::Normal
                    }
                } else {
                    wt_core::PiecePriority::None
                };
                file.set_priority(priority);
            }
            if active {
                rec.torrent.allow_data_download();
                rec.torrent.allow_data_upload();
            }
        }
        self.apply_seed_policy(rec);
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
            Arc::new(move |id: &str, file_index: usize| this.open_playback(id, file_index).ok());

        let server = start_stream_server(0, resolver).await?;
        self.stream_port
            .store(server.port() as usize, Ordering::SeqCst);
        *self.stream_server.lock() = Some(server);
        self.notify_change();
        Ok(())
    }

    pub fn close(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let mut contacts = wt_storage::DhtContactStore::new();
        contacts.load(&self.download_dir);
        for addr in self.client.dht_contacts() {
            contacts.remember_dht_contact(&self.download_dir, &addr.to_string(), None);
        }
        self.client.close();
        if let Some(server) = self.stream_server.lock().take() {
            server.close();
        }
        let mut readers = self.playback_readers.lock();
        for (_, r) in readers.drain() {
            r.close();
        }
        let mut preview = self.preview_active.lock();
        if let Some(p) = preview.take() {
            p.close();
        }
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

        match (method, parts.as_slice()) {
            ("GET", ["stats"]) => self.handle_stats(),
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
        }
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
        if let Some(ref b64) = req.torrent_data {
            if let Ok(bytes) =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
            {
                torrent_data_bytes = Some(bytes);
            }
        }

        let mut spec = wt_core::TorrentSpec::default();
        let mut peers = Vec::new();
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
            spec.trackers = m.trackers.into_iter().map(|v| vec![v]).collect();
            spec.web_seeds = m.web_seeds;
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
        let id = info_hash.clone();
        let info = torrent.info();
        let files: Vec<FileView> = torrent
            .files()
            .into_iter()
            .map(|file| {
                let path = file.path();
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                FileView {
                    index: file.index(),
                    name,
                    path,
                    length: file.length(),
                    progress: 0.0,
                    verified_bytes: file.bytes_completed(),
                    file_type: "application/octet-stream".into(),
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
        let rec = Arc::new(RwLock::new(TorrentRecord {
            torrent: torrent.clone(),
            playback_coordinator: Arc::new(crate::playback::PlaybackPriorityCoordinator::new()),
            playback_demand: HashSet::new(),
            id: id.clone(),
            info_hash: info_hash.clone(),
            name: info.as_ref().map(|i| i.name.clone()),
            magnet_uri: Some(format!("magnet:?xt=urn:btih:{info_hash}")),
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
            verified_bytes: 0,
            piece_verified: vec![false; total_pieces],
            next_preload: None,
            playback_readers: 0,
            resting: false,
            idle_drop_ready: false,
        }));

        self.records.write().insert(id.clone(), rec.clone());
        self.records_by_hash.write().insert(info_hash.clone(), rec);
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
        let trackers = build_tracker_views(&rec.original_trackers, &[], &live_status);

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
                selected_verified as f64 / selected_total as f64
            } else {
                0.0
            },
            download_speed: 0,
            upload_speed: 0,
            num_peers: rec.torrent.stats().total_peers,
            length: selected_total,
            downloaded: selected_verified,
            verified_bytes: selected_verified,
            uploaded: rec.uploaded,
            files: rec.files.clone(),
            trackers,
            error: rec.error.clone(),
            time_remaining: None,
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
            next_preload: rec.next_preload.clone(),
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

        let records = self.records.read();
        let rec = match records.get(&req.id) {
            Some(r) => {
                let mut rec = r.write();
                rec.refresh();
                rec
            }
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                )
            }
        };

        let file_index = req.file_index.unwrap_or(0);
        if file_index >= rec.files.len() {
            return (
                400,
                serde_json::to_vec(&json!({"error": "invalid file index"})).unwrap(),
            );
        }

        let file = &rec.files[file_index];
        if !self.payload_transfers_allowed.load(Ordering::SeqCst)
            && file.verified_bytes < file.length
        {
            return (
                409,
                serde_json::to_vec(&json!({"error": "payload transfers are disabled in settings"}))
                    .unwrap(),
            );
        }
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
                .get("fileOnly")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut rec = r.write();
            rec.file_only = enabled;
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
        let records = self.records.read();
        if let Some(r) = records.get(&req.id) {
            let mut rec = r.write();
            let Some(storage) = self.storage.get_storage(&rec.info_hash) else {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":"metadata not ready"})).unwrap(),
                );
            };
            let stream = req.storage_mode.as_deref() == Some(STORAGE_MODE_STREAM);
            if let Err(e) = storage.attach_descriptors_mode(&req.descriptors, &req.selected, stream)
            {
                return (
                    400,
                    serde_json::to_vec(&json!({"error":e.to_string()})).unwrap(),
                );
            }
            for piece in 0..rec.torrent.num_pieces() {
                rec.torrent.update_piece_completion(piece);
            }
            rec.configured = true;
            rec.selected = req.selected;
            if req.storage_mode.as_deref() == Some(STORAGE_MODE_STREAM) {
                rec.direct_play = true;
            }
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
        let records = self.records.read();
        if let Some(r) = records.get(&req.id) {
            let mut rec = r.write();
            rec.selected = req.selected.clone();
            self.apply_transfer_state(&mut rec);
            self.notify_change();
            (
                200,
                serde_json::to_vec(&SelectResponse {
                    ok: true,
                    selected: req.selected,
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
        let records = self.records.read();
        if let Some(r) = records.get(&req.id) {
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
        let mut records = self.records.write();
        let mut by_hash = self.records_by_hash.write();
        if let Some(rec) = records.remove(&req.id) {
            rec.read().torrent.drop_torrent();
            if req.destroy_store.unwrap_or(false) {
                let _ = self.storage.remove_torrent(rec.read().torrent.info_hash());
            }
            by_hash.remove(&rec.read().info_hash);
            self.tracker_status.clear_status(&rec.read().info_hash);
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
            if let Some(r) = self.records_by_hash.read().get(hash) {
                r.read().torrent.forget_peers();
            }
        }
        (200, serde_json::to_vec(&json!({"ok":true})).unwrap())
    }

    fn handle_pieces(&self, id: &str, query: &str) -> (u16, Vec<u8>) {
        let records = self.records.read();
        let rec = match records.get(id) {
            Some(r) => {
                let mut rec = r.write();
                rec.refresh();
                rec
            }
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                )
            }
        };

        let mut max_buckets = 32;
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "maxBuckets" {
                    if let Ok(n) = v.parse::<usize>() {
                        max_buckets = n.clamp(8, 64);
                    }
                }
            }
        }

        let total_pieces = rec.total_pieces;
        let buckets_count = max_buckets.min(total_pieces.max(1));
        let mut buckets = Vec::new();
        let pieces_per_bucket = (total_pieces + buckets_count - 1) / buckets_count.max(1);

        let piece_len = if rec.piece_length > 0 {
            rec.piece_length
        } else {
            1
        };
        let mut selected_pieces = vec![false; total_pieces];
        let files = rec.torrent.files();
        for &idx in &rec.selected {
            if let Some(f) = files.get(idx) {
                if f.length() == 0 {
                    continue;
                }
                let start_piece = (f.offset() / piece_len) as usize;
                let end_piece = ((f.offset() + f.length() + piece_len - 1) / piece_len) as usize;
                for item in selected_pieces
                    .iter_mut()
                    .take(end_piece.min(total_pieces))
                    .skip(start_piece)
                {
                    *item = true;
                }
            }
        }

        for i in 0..buckets_count {
            let start = i * pieces_per_bucket;
            let end = ((i + 1) * pieces_per_bucket).min(total_pieces);
            let total = end.saturating_sub(start);
            let mut selected = 0;
            let mut verified = 0;
            let mut selected_verified = 0;
            for p in start..end {
                let is_sel = selected_pieces.get(p).copied().unwrap_or(false);
                let is_ver = rec.piece_verified.get(p).copied().unwrap_or(false);
                if is_sel {
                    selected += 1;
                }
                if is_ver {
                    verified += 1;
                    if is_sel {
                        selected_verified += 1;
                    }
                }
            }
            buckets.push(PieceBucket {
                start,
                end,
                total,
                selected,
                verified,
                receiving: 0,
                selected_verified,
                selected_receiving: 0,
            });
        }

        let resp = PieceTelemetryResponse {
            id: rec.id.clone(),
            info_hash: Some(rec.info_hash.clone()),
            generation: 1,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            total_pieces,
            piece_length: rec.piece_length,
            last_piece_length: rec.last_piece_length,
            max_buckets,
            buckets,
            ghost_pro_mode: self.ghost_pro_mode.load(Ordering::SeqCst),
            playback: None,
            playheads: None,
            playback_detail: None,
        };

        (200, serde_json::to_vec(&resp).unwrap())
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
                active: false,
                byte_offset: 0,
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
                });
            }
        }
        sort_peer_connections(&mut peers);

        let resp = ConnectionsResponse {
            id: rec.id.clone(),
            paused: rec.paused,
            peers,
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
            effective_dial_rate_per_sec: self.dial_rate_per_sec.load(Ordering::SeqCst) as i64,
            peer_exploration: self.peer_exploration.load(Ordering::SeqCst),
            claude_seek_mode: self.claude_seek_mode.load(Ordering::SeqCst),
            hedge_requests: self.hedge_requests.load(Ordering::SeqCst),
            slow_peer_yield: self.slow_peer_yield.load(Ordering::SeqCst),
            drop_chokers: self.drop_chokers.load(Ordering::SeqCst),
            learned_arrival: self.learned_arrival.load(Ordering::SeqCst),
            arrival: ArrivalStats::default(),
            idle_drop_minutes: self.idle_drop_minutes.load(Ordering::SeqCst),
            ram_cache_bytes: self.ram_cache_bytes.load(Ordering::SeqCst),
            seeding_enabled: self.seeding_enabled.load(Ordering::SeqCst),
            seed_ratio_milli: self.seed_ratio_milli.load(Ordering::SeqCst),
            seed_time_minutes: self.seed_time_minutes.load(Ordering::SeqCst),
            max_peers: self.max_peers.load(Ordering::SeqCst),
            download_rate_bytes_sec: self.download_rate_limit.load(Ordering::SeqCst),
            upload_rate_bytes_sec: self.upload_rate_limit.load(Ordering::SeqCst),
            payload_transfers_allowed: self.payload_transfers_allowed.load(Ordering::SeqCst),
            seek_stats: self.seek_stats.lock().clone(),
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

        if let Some(peers) = req.max_peers.filter(|&p| p != 0) {
            if !(8..=500).contains(&peers) {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "maxPeers must be between 8 and 500"}))
                        .unwrap(),
                );
            }
            self.max_peers.store(peers, Ordering::SeqCst);
            for t in self.client.torrents() {
                t.set_max_established_conns(peers);
            }
        }

        if let Some(cache_bytes) = req.ram_cache_bytes {
            if !(16 * 1024 * 1024..=1024 * 1024 * 1024).contains(&cache_bytes) {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "ramCacheBytes out of bounds"})).unwrap(),
                );
            }
            self.ram_cache_bytes.store(cache_bytes, Ordering::SeqCst);
            self.storage.set_memory_limit(cache_bytes);
        }

        if let Some(dial) = req.dial_rate_per_sec {
            let clamped = dial.clamp(10, 80);
            self.dial_rate_per_sec.store(clamped, Ordering::SeqCst);
            self.client.inner.dial_limiter.set_limit(clamped as f64);
        }

        if let Some(ghost) = req.ghost_mode {
            self.ghost_mode.store(ghost, Ordering::SeqCst);
        }
        if let Some(pro) = req.ghost_pro_mode {
            self.ghost_pro_mode.store(pro, Ordering::SeqCst);
        }
        if let Some(exp) = req.peer_exploration {
            self.peer_exploration.store(exp, Ordering::SeqCst);
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
                *self.seek_stats.lock() = SeekStats::default();
            }
        }
        if let Some(h) = req.hedge_requests {
            self.hedge_requests.store(h, Ordering::SeqCst);
        }
        if let Some(s) = req.slow_peer_yield {
            self.slow_peer_yield.store(s, Ordering::SeqCst);
        }
        if let Some(d) = req.drop_chokers {
            self.drop_chokers.store(d, Ordering::SeqCst);
        }
        if let Some(la) = req.learned_arrival {
            self.learned_arrival.store(la, Ordering::SeqCst);
        }
        if let Some(idle) = req.idle_drop_minutes {
            self.idle_drop_minutes.store(idle, Ordering::SeqCst);
        }
        if let Some(se) = req.seeding_enabled {
            self.seeding_enabled.store(se, Ordering::SeqCst);
        }
        if let Some(sa) = req.seeding_allowed {
            self.seeding_allowed.store(sa, Ordering::SeqCst);
        }
        if let Some(ratio) = req.seed_ratio_milli {
            self.seed_ratio_milli.store(ratio, Ordering::SeqCst);
        }
        if let Some(mins) = req.seed_time_minutes {
            self.seed_time_minutes.store(mins, Ordering::SeqCst);
        }
        if let Some(down) = req.download_rate_bytes_sec {
            self.download_rate_limit.store(down, Ordering::SeqCst);
            self.client.inner.download_limiter.set_limit(if down > 0 {
                down as f64
            } else {
                f64::INFINITY
            });
        }
        if let Some(up) = req.upload_rate_bytes_sec {
            self.upload_rate_limit.store(up, Ordering::SeqCst);
            self.client.inner.upload_limiter.set_limit(if up > 0 {
                up as f64
            } else {
                f64::INFINITY
            });
        }
        if let Some(allowed) = req.payload_transfers_allowed {
            self.payload_transfers_allowed
                .store(allowed, Ordering::SeqCst);
        }

        for r in self.records.read().values() {
            let mut rec = r.write();
            self.apply_transfer_state(&mut rec);
        }
        self.notify_change();
        self.handle_get_settings()
    }

    fn handle_network_change(&self, body: &str) -> (u16, Vec<u8>) {
        let (generation, connected) = match NetworkManager::parse_change_body(body) {
            Ok(v) => v,
            Err(e) => return (400, serde_json::to_vec(&json!({"error":e})).unwrap()),
        };
        let _ = self.network.on_network_change(generation, connected);
        self.notify_change();
        (
            200,
            serde_json::to_vec(&OkResponse { ok: true, id: None }).unwrap(),
        )
    }

    fn handle_get_bypass(&self) -> (u16, Vec<u8>) {
        let enabled = self.bypass_enabled.load(Ordering::SeqCst);
        let status = BypassStatus {
            enabled,
            active: enabled,
            mode: if enabled {
                "tcp-write-split".to_string()
            } else {
                "off".to_string()
            },
            coverage: if enabled {
                Some(vec![
                    "outgoing-tcp-peers".into(),
                    "http-trackers".into(),
                    "websocket-trackers".into(),
                ])
            } else {
                None
            },
            error: None,
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

    fn handle_get_cache(&self) -> (u16, Vec<u8>) {
        let resp = CacheStatusResponse {
            categories: vec![
                CacheCategoryView {
                    id: "startup".to_string(),
                    bytes: 0,
                    reclaimable_bytes: 0,
                },
                CacheCategoryView {
                    id: "resume".into(),
                    bytes: 0,
                    reclaimable_bytes: 0,
                },
                CacheCategoryView {
                    id: "peers".into(),
                    bytes: 0,
                    reclaimable_bytes: 0,
                },
                CacheCategoryView {
                    id: "dht".into(),
                    bytes: 0,
                    reclaimable_bytes: 0,
                },
            ],
            memory_bytes: self.ram_cache_bytes.load(Ordering::SeqCst),
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
        match wt_storage::clear_cache_categories(&self.download_dir, &categories) {
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
        let coordinator = self
            .records
            .read()
            .get(id)
            .map(|r| r.read().playback_coordinator.clone())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "torrent not found")
            })?;
        let host = self
            .self_ref
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| std::io::Error::other("engine closed"))?;
        let handle = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        let (_, reader) = PlaybackReader::open_for_host(host, coordinator, id, index, 0, handle)?;
        let mut readers = self.playback_readers.lock();
        readers.retain(|_, reader| !reader.is_closed());
        readers.insert(handle, reader.clone());
        Ok(reader)
    }

    pub fn open_preview(&self, id: &str, index: usize) -> std::io::Result<Arc<PreviewReader>> {
        let mut active = self.preview_active.lock();
        if active.as_ref().is_some_and(|reader| reader.is_closed()) {
            *active = None;
        }
        if active.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "preview already active",
            ));
        }

        let records = self.records.read();
        let mut rec = records
            .get(id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "torrent not found"))?
            .write();
        rec.refresh();

        if index >= rec.files.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid file index",
            ));
        }

        let file =
            rec.torrent.files().get(index).cloned().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "file not found")
            })?;
        let length = file.length();
        let backend = Mutex::new(crate::playback_core::CoreReaderBackend::from_file(&file));
        let reader = Arc::new(PreviewReader::new(
            index,
            length,
            Box::new(move |buf, offset| {
                crate::playback::ReaderBackend::read_at(&mut *backend.lock(), buf, offset)
            }),
        ));

        *active = Some(reader.clone());
        let handle = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        self.preview_readers.lock().insert(handle, reader.clone());
        Ok(reader)
    }
}

impl crate::playback::PlaybackHost for EngineServer {
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
            open_reader_count: rec.playback_readers,
        })
    }
    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }
    fn payload_allowed(&self) -> bool {
        self.payload_transfers_allowed.load(Ordering::SeqCst)
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
        Ok(Box::new(
            crate::playback_core::CoreReaderBackend::from_file(&file),
        ))
    }
    fn apply_piece_priorities(&self, id: &str, demand: &HashMap<u32, wt_core::PiecePriority>) {
        if let Some(r) = self.records.read().get(id) {
            let mut rec = r.write();
            let previous = std::mem::take(&mut rec.playback_demand);
            let info = rec.torrent.info();
            for piece in previous {
                if demand.contains_key(&piece) {
                    continue;
                }
                let wanted = info.as_ref().is_some_and(|info| {
                    let p = info.piece(piece);
                    rec.torrent.files().iter().any(|f| {
                        rec.selected.contains(&f.index())
                            && f.offset() < p.offset + p.length
                            && f.offset() + f.length() > p.offset
                    })
                });
                rec.torrent.set_piece_priority(
                    piece,
                    if wanted {
                        wt_core::PiecePriority::Normal
                    } else {
                        wt_core::PiecePriority::None
                    },
                );
            }
            for (&piece, &priority) in demand {
                rec.torrent.set_piece_priority(piece, priority);
            }
            rec.playback_demand.extend(demand.keys().copied());
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
            r.read().torrent.refocus_requests();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_stats_and_version() {
        let server = EngineServer::new(0, "/tmp/wt-test-stats", 40).expect("init server");
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
        let server = EngineServer::new(0, "/tmp/wt-test-lifecycle", 40).expect("init server");
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
        let server = EngineServer::new(0, "/tmp/wt-test-cfg", 40).expect("init server");
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

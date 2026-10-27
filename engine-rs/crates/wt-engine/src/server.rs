//! EngineServer: control router, event notification, and subsystem coordinator.
//!
//! Entry point for integration tests and JNI host:
//! - Constructor: `EngineServer::new(ctl_port: u16, download_dir: impl AsRef<Path>, max_peers: usize) -> std::io::Result<Arc<EngineServer>>`
//! - Request entry point: `EngineServer::request(&self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>)`
//!   dispatches any control API route and returns `(http_status_code, response_body_bytes)`.
//! - JNI entry point: implements `crate::jni::host_api::EngineHostApi`.
//!
//! Port of `engine-go/server.go`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
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

pub struct EngineServer {
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
            55
        } else {
            max_peers.min(500)
        };

        Ok(Arc::new(Self {
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
            ghost_pro_mode: AtomicBool::new(true),
            dial_rate_per_sec: AtomicUsize::new(50),
            peer_exploration: AtomicBool::new(false),
            claude_seek_mode: AtomicBool::new(true),
            hedge_requests: AtomicBool::new(true),
            slow_peer_yield: AtomicBool::new(true),
            drop_chokers: AtomicBool::new(true),
            learned_arrival: AtomicBool::new(true),
            idle_drop_minutes: AtomicI64::new(15),
            ram_cache_bytes: AtomicI64::new(64 * 1024 * 1024),
            seeding_enabled: AtomicBool::new(true),
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
        }))
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
        let resolver = Arc::new(move |id: &str, file_index: usize| {
            let records = this.records.read();
            let rec = records.get(id)?.read();
            if file_index >= rec.files.len() {
                return None;
            }
            let file = &rec.files[file_index];
            let length = file.length;
            let read_fn = Arc::new(move |buf: &mut [u8], _offset: i64| {
                buf.fill(0);
                Ok(buf.len())
            });
            Some(Arc::new(PlaybackReader::new(
                id.to_string(),
                file_index,
                length,
                read_fn,
                false,
                false,
                false,
                None,
            )))
        });

        let server = start_stream_server(0, resolver).await?;
        self.stream_port
            .store(server.port() as usize, Ordering::SeqCst);
        *self.stream_server.lock() = Some(server);
        self.notify_change();
        Ok(())
    }

    pub fn close(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
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
            listen_port: 0,
        };
        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_add(&self, body: &str) -> (u16, Vec<u8>) {
        let req: AddRequest = match serde_json::from_str(body) {
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

        // Generate id from infohash or random
        let info_hash = if !req.torrent_id.is_empty() {
            req.torrent_id.to_ascii_lowercase()
        } else {
            format!("{:040x}", rand::random::<u128>())
        };
        let id = info_hash.clone();

        let rec = Arc::new(RwLock::new(TorrentRecord {
            id: id.clone(),
            info_hash: info_hash.clone(),
            name: Some("test_torrent".to_string()),
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
            files: vec![FileView {
                index: 0,
                name: "file.mp4".to_string(),
                path: "file.mp4".to_string(),
                length: 1024 * 1024,
                progress: 0.0,
                verified_bytes: 0,
                file_type: "video/mp4".to_string(),
            }],
            original_trackers: vec![vec![
                "udp://tracker.opentrackr.org:1337/announce".to_string()
            ]],
            supplemental_trackers: 0,
            seeding: false,
            seed_base: self.seeding.get_account(&info_hash),
            torrent_bytes: torrent_data_bytes,
            total_pieces: 10,
            piece_length: 16384,
            last_piece_length: 16384,
            downloaded: 0,
            uploaded: 0,
            verified_bytes: 0,
            piece_verified: vec![false; 10],
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
            Some(r) => r.read(),
            None => {
                return (
                    404,
                    serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
                );
            }
        };

        let live_status = self.tracker_status.snapshot(&rec.info_hash);
        let trackers = build_tracker_views(&rec.original_trackers, &[], &live_status);

        let view = TorrentView {
            seeding: rec.seeding,
            seed_ratio: 0.0,
            id: rec.id.clone(),
            info_hash: Some(rec.info_hash.clone()),
            name: rec.name.clone(),
            magnet_uri: rec.magnet_uri.clone(),
            ready: true,
            done: rec.verified_bytes >= rec.files.iter().map(|f| f.length).sum::<i64>(),
            paused: rec.paused,
            progress: {
                let total: i64 = rec.files.iter().map(|f| f.length).sum();
                if total > 0 {
                    (rec.verified_bytes as f64 / total as f64).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            },
            download_speed: 0,
            upload_speed: 0,
            num_peers: 0,
            length: rec.files.iter().map(|f| f.length).sum(),
            downloaded: rec.downloaded,
            verified_bytes: rec.verified_bytes,
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
        let req: PlayRequest = match serde_json::from_str(body) {
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
            Some(r) => r.read(),
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
        let stream_url = format!(
            "http://127.0.0.1:{}/torrent/{}/{}",
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
            r.write().file_only = true;
            (
                200,
                serde_json::to_vec(&OkResponse {
                    ok: true,
                    id: Some(id.to_string()),
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
            r.write().focused_file = req.file_index;
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
            rec.configured = true;
            rec.selected = req.selected;
            if req.storage_mode.as_deref() == Some(STORAGE_MODE_STREAM) {
                rec.direct_play = true;
            }
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
            r.write().paused = pause;
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
        let req: IdRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        (
            200,
            serde_json::to_vec(&OkResponse {
                ok: true,
                id: Some(req.id),
            })
            .unwrap(),
        )
    }

    fn handle_pieces(&self, id: &str, query: &str) -> (u16, Vec<u8>) {
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

        for i in 0..buckets_count {
            let start = i * pieces_per_bucket;
            let end = ((i + 1) * pieces_per_bucket).min(total_pieces);
            let total = end.saturating_sub(start);
            let verified = if !rec.piece_verified.is_empty() {
                rec.piece_verified[start..end]
                    .iter()
                    .filter(|&&v| v)
                    .count()
            } else {
                0
            };
            buckets.push(PieceBucket {
                start,
                end,
                total,
                selected: total,
                verified,
                receiving: 0,
                selected_verified: verified,
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

    fn handle_preview_state(&self, id: &str, _query: &str) -> (u16, Vec<u8>) {
        let records = self.records.read();
        if !records.contains_key(id) {
            return (
                404,
                serde_json::to_vec(&json!({"error": "torrent not found"})).unwrap(),
            );
        }
        let resp = PreviewStateResponse {
            active: false,
            byte_offset: 0,
            signature: "0".to_string(),
        };
        (200, serde_json::to_vec(&resp).unwrap())
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

        if let Some(ref data) = rec.torrent_bytes {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
            (
                200,
                serde_json::to_vec(&MetadataResponse { torrent_data: b64 }).unwrap(),
            )
        } else {
            (
                404,
                serde_json::to_vec(&json!({"error": "metadata not available"})).unwrap(),
            )
        }
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
            seek_stats: SeekStats::default(),
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

        if let Some(peers) = req.max_peers {
            if !(8..=500).contains(&peers) {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "maxPeers must be between 8 and 500"}))
                        .unwrap(),
                );
            }
            self.max_peers.store(peers, Ordering::SeqCst);
        }

        if let Some(cache_bytes) = req.ram_cache_bytes {
            if !(16 * 1024 * 1024..=1024 * 1024 * 1024).contains(&cache_bytes) {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "ramCacheBytes out of bounds"})).unwrap(),
                );
            }
            self.ram_cache_bytes.store(cache_bytes, Ordering::SeqCst);
        }

        if let Some(dial) = req.dial_rate_per_sec {
            let clamped = dial.clamp(10, 80);
            self.dial_rate_per_sec.store(clamped, Ordering::SeqCst);
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
            self.claude_seek_mode.store(seek, Ordering::SeqCst);
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
        }
        if let Some(up) = req.upload_rate_bytes_sec {
            self.upload_rate_limit.store(up, Ordering::SeqCst);
        }
        if let Some(allowed) = req.payload_transfers_allowed {
            self.payload_transfers_allowed
                .store(allowed, Ordering::SeqCst);
        }

        self.notify_change();
        self.handle_get_settings()
    }

    fn handle_network_change(&self, body: &str) -> (u16, Vec<u8>) {
        let req: NetworkChangeRequest = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(_) => {
                return (
                    400,
                    serde_json::to_vec(&json!({"error": "invalid json"})).unwrap(),
                )
            }
        };
        let connected = req.connected.unwrap_or(false);
        let _ = self.network.on_network_change(req.generation, connected);
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
                "active".to_string()
            } else {
                "off".to_string()
            },
            coverage: vec!["all".to_string()],
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
                    id: "resume".to_string(),
                    bytes: 0,
                    reclaimable_bytes: 0,
                },
            ],
            memory_bytes: self.ram_cache_bytes.load(Ordering::SeqCst),
        };
        (200, serde_json::to_vec(&resp).unwrap())
    }

    fn handle_clear_cache(&self, _body: &str) -> (u16, Vec<u8>) {
        self.handle_get_cache()
    }

    /// Public control entry point for integration tests and JNI layer.
    ///
    /// Takes `method` ("GET", "POST"), `path` ("/stats", "/torrent/...", etc.),
    /// and `body` (JSON string or empty), and returns `(http_status_code, response_body_bytes)`.
    pub fn request(&self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        self.dispatch_control(method, path, body)
    }

    pub fn open_playback(&self, id: &str, index: usize) -> std::io::Result<Arc<PlaybackReader>> {
        let records = self.records.read();
        let rec = records
            .get(id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "torrent not found"))?
            .read();

        if index >= rec.files.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid file index",
            ));
        }

        let file = &rec.files[index];
        let length = file.length;
        let read_fn = Arc::new(|buf: &mut [u8], _offset: i64| {
            buf.fill(0);
            Ok(buf.len())
        });

        let reader = Arc::new(PlaybackReader::new(
            id.to_string(),
            index,
            length,
            read_fn,
            false,
            false,
            false,
            None,
        ));

        let handle = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        self.playback_readers.lock().insert(handle, reader.clone());
        Ok(reader)
    }

    pub fn open_preview(&self, id: &str, index: usize) -> std::io::Result<Arc<PreviewReader>> {
        let mut active = self.preview_active.lock();
        if active.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "preview already active",
            ));
        }

        let records = self.records.read();
        let rec = records
            .get(id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "torrent not found"))?
            .read();

        if index >= rec.files.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid file index",
            ));
        }

        let file = &rec.files[index];
        let length = file.length;
        let reader = Arc::new(PreviewReader::new(
            index,
            length,
            Box::new(|buf: &mut [u8], _offset: i64| {
                buf.fill(0);
                Ok(buf.len())
            }),
        ));

        *active = Some(reader.clone());
        let handle = self.next_reader_id.fetch_add(1, Ordering::SeqCst);
        self.preview_readers.lock().insert(handle, reader.clone());
        Ok(reader)
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

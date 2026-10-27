//! `EngineHostApi` backed by [`crate::server::EngineServer`].
//!
//! Carries over the Android JNI behaviour previously sketched in
//! `jni_export.rs`: tokio runtime for `start_streaming`, control framing,
//! playback/preview handle tables, and Go-compatible error codes.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::playback::PlaybackReader;
use crate::preview::PreviewReader;
use crate::server::EngineServer;

use super::host_api::EngineHostApi;
use super::{log_error, log_info};

/// Android/JNI host wrapping a single [`EngineServer`].
pub struct AndroidEngineHost {
    server: Mutex<Option<Arc<EngineServer>>>,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    playback: Mutex<HashMap<i64, Arc<PlaybackReader>>>,
    preview: Mutex<HashMap<i64, Arc<PreviewReader>>>,
}

impl AndroidEngineHost {
    pub fn new() -> Self {
        Self {
            server: Mutex::new(None),
            runtime: Mutex::new(None),
            playback: Mutex::new(HashMap::new()),
            preview: Mutex::new(HashMap::new()),
        }
    }

    fn current_server(&self) -> Option<Arc<EngineServer>> {
        self.server.lock().clone()
    }
}

impl Default for AndroidEngineHost {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineHostApi for AndroidEngineHost {
    fn start_engine(&self, download_dir: &[u8], max_peers: i32) -> i32 {
        let mut lock = self.server.lock();
        if lock.is_some() {
            return 0;
        }

        if download_dir.is_empty() {
            log_error("download directory is empty");
            return 1;
        }

        let download_dir = match std::str::from_utf8(download_dir) {
            Ok(s) => s.to_string(),
            Err(_) => {
                log_error("download directory is not valid UTF-8");
                return 1;
            }
        };

        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("wt-engine")
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                log_error(&format!("Failed to create tokio runtime: {err}"));
                return 1;
            }
        };

        let peers = if max_peers <= 0 {
            55
        } else {
            max_peers as usize
        };

        let server = match EngineServer::new(0, &download_dir, peers) {
            Ok(server) => server,
            Err(err) => {
                log_error(&format!("Failed to create engine: {err}"));
                return 1;
            }
        };

        let streaming = server.clone();
        if let Err(err) = rt.block_on(async move { streaming.start_streaming().await }) {
            server.close();
            log_error(&format!("Failed to start media server: {err}"));
            return 1;
        }

        let stream_port = server.stream_port.load(Ordering::SeqCst);
        server.notify_change();
        *lock = Some(server);
        *self.runtime.lock() = Some(rt);

        log_info(&format!(
            "Engine ready: JNI control, streamPort={stream_port}"
        ));
        0
    }

    fn request_engine(&self, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
        let Some(server) = self.current_server() else {
            return b"503\n{\"error\":\"engine is starting\"}".to_vec();
        };
        let body = String::from_utf8_lossy(body);
        let (status, response) = server.dispatch_control(method, path, body.as_ref());
        let mut payload = format!("{status}\n").into_bytes();
        payload.extend_from_slice(&response);
        payload
    }

    fn wait_for_engine_event(&self, after_version: i64, timeout_millis: i64) -> i64 {
        let Some(server) = self.current_server() else {
            std::thread::sleep(Duration::from_millis(250));
            return after_version;
        };
        let mut timeout = Duration::from_millis(timeout_millis.max(0) as u64);
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            timeout = Duration::from_secs(30);
        }
        server.wait_for_change(after_version as u64, timeout) as i64
    }

    fn open_playback(&self, id: &str, file_index: i32) -> i64 {
        let Some(server) = self.current_server() else {
            return 0;
        };
        if file_index < 0 {
            return 0;
        }
        let index = file_index as usize;
        // Open via public EngineServer fields so we stay resilient while
        // agy-4 settles open_playback's return type (Arc vs (handle, Arc)).
        let length = {
            let records = server.records.read();
            let Some(rec) = records.get(id) else {
                log_error("torrent not found");
                return 0;
            };
            let rec = rec.read();
            if index >= rec.files.len() {
                log_error("invalid file index");
                return 0;
            }
            rec.files[index].length
        };
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
        let handle = server.next_reader_id.fetch_add(1, Ordering::SeqCst);
        server
            .playback_readers
            .lock()
            .insert(handle, reader.clone());
        self.playback.lock().insert(handle, reader);
        handle
    }

    fn read_playback(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32 {
        if buf.len() > 1024 * 1024 {
            return -1;
        }
        let reader = {
            let map = self.playback.lock();
            match map.get(&handle) {
                Some(reader) => reader.clone(),
                None => return -1,
            }
        };
        match reader.read_at(buf, offset) {
            Ok(n) => n as i32,
            Err(err) => {
                log_error(&format!("playback read: {err}"));
                -1
            }
        }
    }

    fn close_playback(&self, handle: i64) {
        if let Some(reader) = self.playback.lock().remove(&handle) {
            reader.close();
        }
        if let Some(server) = self.current_server() {
            if let Some(reader) = server.playback_readers.lock().remove(&handle) {
                reader.close();
            }
        }
    }

    fn open_preview(&self, id: &str, file_index: i32) -> i64 {
        let Some(server) = self.current_server() else {
            return 0;
        };
        if file_index < 0 {
            return 0;
        }
        let index = file_index as usize;
        {
            let active = server.preview_active.lock();
            if active.is_some() {
                log_error("preview already active");
                return 0;
            }
        }
        let length = {
            let records = server.records.read();
            let Some(rec) = records.get(id) else {
                log_error("torrent not found");
                return 0;
            };
            let rec = rec.read();
            if index >= rec.files.len() {
                log_error("invalid file index");
                return 0;
            }
            rec.files[index].length
        };
        let reader = Arc::new(PreviewReader::new(
            index,
            length,
            Box::new(|buf: &mut [u8], _offset: i64| {
                buf.fill(0);
                Ok(buf.len())
            }),
        ));
        *server.preview_active.lock() = Some(reader.clone());
        let handle = server.next_reader_id.fetch_add(1, Ordering::SeqCst);
        server.preview_readers.lock().insert(handle, reader.clone());
        self.preview.lock().insert(handle, reader);
        handle
    }

    fn read_preview(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32 {
        if buf.len() > 1024 * 1024 {
            return -1;
        }
        let reader = {
            let map = self.preview.lock();
            match map.get(&handle) {
                Some(reader) => reader.clone(),
                None => return -1,
            }
        };
        match reader.read_at(buf, offset) {
            Ok(n) => {
                if (n as i32) < 0 {
                    -1
                } else {
                    n as i32
                }
            }
            Err(err) => {
                log_error(&format!("preview read: {err}"));
                -1
            }
        }
    }

    fn close_preview(&self, handle: i64) {
        if let Some(reader) = self.preview.lock().remove(&handle) {
            reader.close();
        }
        if let Some(server) = self.current_server() {
            if let Some(reader) = server.preview_readers.lock().remove(&handle) {
                reader.close();
            }
            *server.preview_active.lock() = None;
        }
    }
}

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
        let reader = match server.open_playback(id, file_index as usize) {
            Ok(reader) => reader,
            Err(err) => {
                log_error(&format!("open playback: {err}"));
                return 0;
            }
        };
        let handle = match server
            .playback_readers
            .lock()
            .iter()
            .find_map(|(&handle, candidate)| Arc::ptr_eq(candidate, &reader).then_some(handle))
        {
            Some(handle) => handle,
            None => return 0,
        };
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
        // ProxyFileDescriptorCallback must return the exact size asked for unless
        // the file ends. AppFuse opens files without direct I/O, so the kernel
        // takes a short read for the end of the file and shrinks it, and the
        // player sees a truncated video.
        match reader.read_at(buf, offset) {
            Ok(n) => n as i32,
            Err(err) => {
                log_error(&format!("playback read: {err}"));
                -1
            }
        }
    }

    fn close_playback(&self, handle: i64) {
        let reader = self.playback.lock().remove(&handle);
        if let Some(reader) = reader {
            reader.close();
        }
        if let Some(server) = self.current_server() {
            let reader = server.playback_readers.lock().remove(&handle);
            if let Some(reader) = reader {
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
        let reader = match server.open_preview(id, file_index as usize) {
            Ok(reader) => reader,
            Err(err) => {
                log_error(&format!("open preview: {err}"));
                return 0;
            }
        };
        let handle = match server
            .preview_readers
            .lock()
            .iter()
            .find_map(|(&handle, candidate)| Arc::ptr_eq(candidate, &reader).then_some(handle))
        {
            Some(handle) => handle,
            None => return 0,
        };
        self.preview.lock().insert(handle, reader);
        handle
    }

    fn read_preview(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32 {
        wt_types::events::seek_trace::note_preview_read();
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
        let reader = self.preview.lock().remove(&handle);
        if let Some(reader) = reader {
            reader.close();
        }
        if let Some(server) = self.current_server() {
            let reader = server.preview_readers.lock().remove(&handle);
            if let Some(reader) = reader {
                reader.close();
            }
        }
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn android_start_engine_restores_contacts_without_a_caller_runtime() {
        let root = "/home/kar/.cache/wt-tmp/codex-engine";
        std::fs::create_dir_all(root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        // Exercise the warm-start path that previously spawned DHT work without
        // a Tokio reactor on the Java caller thread. TEST-NET is not a swarm.
        let mut contacts = wt_storage::DhtContactStore::new();
        contacts.remember_dht_contact(dir.path(), "192.0.2.1:6881", None);
        assert_eq!(contacts.contacts().len(), 1);
        assert!(tokio::runtime::Handle::try_current().is_err());
        for _ in 0..3 {
            let host = AndroidEngineHost::new();
            let path = dir.path().to_str().unwrap().as_bytes();
            assert_eq!(host.start_engine(path, 55), 0);
            assert_eq!(host.start_engine(path, 55), 0, "start must be idempotent");
            for route in ["/stats", "/settings", "/version", "/cache"] {
                let response = host.request_engine("GET", route, b"");
                assert!(
                    response.starts_with(b"200\n"),
                    "{route}: {}",
                    String::from_utf8_lossy(&response)
                );
            }
            let server = host.current_server().unwrap();
            assert!(server.stream_port.load(Ordering::SeqCst) > 0);
            server.close();
        }
    }

    #[test]
    fn invalid_start_engine_arguments_return_errors() {
        let host = AndroidEngineHost::new();
        assert_eq!(host.start_engine(b"", 55), 1);
        assert_eq!(host.start_engine(&[0xff], 55), 1);
        assert!(host.current_server().is_none());
    }
}

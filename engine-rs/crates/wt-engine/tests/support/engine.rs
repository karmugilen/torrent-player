//! Test harness around [`engine::EngineServer`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use engine::types::{SettingsResponse, StatsResponse};
use engine::EngineServer;
use serde::de::DeserializeOwned;
use tempfile::TempDir;

/// Owns a temp download dir and an [`EngineServer`] (Go `testEngine`).
pub struct TestEngine {
    _dir: TempDir,
    pub download_dir: PathBuf,
    pub server: Arc<EngineServer>,
}

impl TestEngine {
    pub fn new() -> Self {
        Self::with_config(|_| {})
    }

    pub fn with_config(customize: impl FnOnce(&mut wt_core::ClientConfig)) -> Self {
        let dir = temp_dir();
        let download_dir = dir.path().to_path_buf();
        let server = EngineServer::new_with_config(0, &download_dir, 55, |cfg| {
            cfg.no_dht = true;
            cfg.disable_utp = true;
            cfg.disable_webtorrent = true;
            cfg.disable_pex = true;
            customize(cfg);
        })
        .expect("EngineServer::new");
        Self {
            _dir: dir,
            download_dir,
            server,
        }
    }

    /// `DispatchControl` equivalent: `(status, body bytes)`.
    pub fn dispatch(&self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        self.server.dispatch_control(method, path, body)
    }

    pub fn dispatch_bytes(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        self.server.dispatch_control_bytes(method, path, body)
    }

    pub fn dispatch_str(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        let (code, raw) = self.dispatch(method, path, body);
        (code, String::from_utf8_lossy(&raw).into_owned())
    }

    pub fn get_settings(&self) -> SettingsResponse {
        let (code, raw) = self.dispatch("GET", "/settings", "");
        assert_eq!(
            code,
            200,
            "GET /settings: {}",
            String::from_utf8_lossy(&raw)
        );
        serde_json::from_slice(&raw).expect("SettingsResponse")
    }

    pub fn post_settings(&self, body: &str) -> SettingsResponse {
        let (code, raw) = self.dispatch("POST", "/settings", body);
        assert_eq!(
            code,
            200,
            "POST /settings: {}",
            String::from_utf8_lossy(&raw)
        );
        serde_json::from_slice(&raw).expect("SettingsResponse")
    }

    pub fn stats(&self) -> StatsResponse {
        let (code, raw) = self.dispatch("GET", "/stats", "");
        assert_eq!(code, 200, "GET /stats: {}", String::from_utf8_lossy(&raw));
        serde_json::from_slice(&raw).expect("StatsResponse")
    }

    pub fn wait_change(&self, after: u64, timeout: Duration) -> u64 {
        self.server.wait_for_change(after, timeout)
    }

    pub fn open_playback(
        &self,
        id: &str,
        file_index: usize,
    ) -> std::io::Result<Arc<engine::playback::PlaybackReader>> {
        self.server.open_playback(id, file_index)
    }

    pub fn open_preview(
        &self,
        id: &str,
        file_index: usize,
    ) -> std::io::Result<Arc<engine::preview::PreviewReader>> {
        self.server.open_preview(id, file_index)
    }

    pub fn close(&self) {
        self.server.close();
    }
}

impl Default for TestEngine {
    fn default() -> Self {
        Self::new()
    }
}

pub fn json_get<T: DeserializeOwned>(eng: &TestEngine, path: &str) -> (u16, T) {
    let (code, raw) = eng.dispatch("GET", path, "");
    let v = serde_json::from_slice(&raw).unwrap_or_else(|e| {
        panic!(
            "decode GET {path} ({code}): {e}; body={}",
            String::from_utf8_lossy(&raw)
        )
    });
    (code, v)
}

pub fn json_post<T: DeserializeOwned>(eng: &TestEngine, path: &str, body: &str) -> (u16, T) {
    let (code, raw) = eng.dispatch("POST", path, body);
    let v = serde_json::from_slice(&raw).unwrap_or_else(|e| {
        panic!(
            "decode POST {path} ({code}): {e}; body={}",
            String::from_utf8_lossy(&raw)
        )
    });
    (code, v)
}

/// All engine fixtures live on disk rather than the RAM-backed system tempdir.
pub fn temp_dir() -> TempDir {
    let root = "/home/kar/.cache/wt-tmp/codex-tests2";
    std::fs::create_dir_all(root).expect("fixture root");
    tempfile::tempdir_in(root).expect("tempdir")
}

impl Drop for TestEngine {
    fn drop(&mut self) {
        self.server.close();
    }
}

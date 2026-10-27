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
        let dir = TempDir::new().expect("tempdir");
        let download_dir = dir.path().to_path_buf();
        let server = EngineServer::new(0, &download_dir, 55).expect("EngineServer::new");
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

    pub fn close(self) {
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

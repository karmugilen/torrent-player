//! Spawn / stop engine host binaries.

use crate::client::{EngineClient, StatsResponse};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub struct EngineProcess {
    pub child: Child,
    pub ctl_port: u16,
    pub data_dir: PathBuf,
    pub label: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl EngineProcess {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn client(&self) -> EngineClient {
        EngineClient::new(self.ctl_port)
    }

    pub fn stats(&self) -> Result<StatsResponse, String> {
        self.client().get_json("/stats")
    }

    pub fn shutdown(mut self) {
        let _ = self.client().request("POST", "/shutdown", Some(b"{}"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

pub fn spawn_engine(
    binary: &Path,
    label: &str,
    ctl_port: u16,
    data_dir: &Path,
    max_peers: i32,
) -> Result<EngineProcess, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let mut child = Command::new(binary)
        .arg("-port")
        .arg(ctl_port.to_string())
        .arg("-path")
        .arg(data_dir)
        .arg("-max-peers")
        .arg(max_peers.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", binary.display()))?;

    let log = Arc::new(Mutex::new(Vec::new()));
    if let Some(out) = child.stdout.take() {
        let log = Arc::clone(&log);
        thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(mut g) = log.lock() {
                    g.push(line);
                }
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let log = Arc::clone(&log);
        thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Ok(mut g) = log.lock() {
                    g.push(line);
                }
            }
        });
    }

    let mut proc = EngineProcess {
        child,
        ctl_port,
        data_dir: data_dir.to_path_buf(),
        label: label.to_string(),
        log,
    };

    // Wait until /stats answers.
    let client = EngineClient::new(ctl_port);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() > deadline {
            let logs = proc.log.lock().map(|g| g.join("\n")).unwrap_or_default();
            let _ = proc.child.kill();
            return Err(format!(
                "engine on :{ctl_port} did not become ready; log:\n{logs}"
            ));
        }
        if let Ok(Some(status)) = proc.child.try_wait() {
            let logs = proc.log.lock().map(|g| g.join("\n")).unwrap_or_default();
            return Err(format!("engine exited early ({status}); log:\n{logs}"));
        }
        if client.get_json::<StatsResponse>("/stats").is_ok() {
            return Ok(proc);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Pick a free localhost TCP port.
pub fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

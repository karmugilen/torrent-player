//! Network route change detection, tracker socket tracking, and recovery.
//!
//! Port of `engine-go/network.go` (+ `networkDialer` dial/reset from
//! `bypass.go`). EngineServer owns the recovery goroutine; this module holds
//! generation state and the dialer that tears down stale HTTP/WebSocket
//! tracker transports when the route changes.

use std::collections::HashSet;
use std::io;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

/// Retry delays used by Go `recoverNetwork` (0, 3s, 10s, 30s).
pub const NETWORK_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(0),
    Duration::from_secs(3),
    Duration::from_secs(10),
    Duration::from_secs(30),
];




/// Tracks HTTP/WebSocket tracker transports so an old route cannot leave a
/// tracker apparently connected after a network change.
pub struct NetworkDialer {
    inner: Mutex<NetworkDialerInner>,
}

struct NetworkDialerInner {
    epoch: u64,
    split: bool,
    connections: HashSet<usize>,
    next_id: usize,
    #[cfg(unix)]
    fds: HashSet<std::os::fd::RawFd>,
}

impl Default for NetworkDialer {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkDialer {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(NetworkDialerInner {
                epoch: 0,
                split: false,
                connections: HashSet::new(),
                next_id: 1,
                #[cfg(unix)]
                fds: HashSet::new(),
            }),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.inner.lock().epoch
    }


    pub fn split(&self) -> bool {
        self.inner.lock().split
    }

    pub fn connection_count(&self) -> usize {
        self.inner.lock().connections.len()
    }

    /// Close every tracked connection and bump the epoch so in-flight dials
    /// that finish later are discarded.
    pub fn reset(&self) {
        let mut g = self.inner.lock();
        g.epoch = g.epoch.wrapping_add(1);
        #[cfg(unix)]
        for fd in g.fds.drain() {
            unsafe {
                libc::shutdown(fd, libc::SHUT_RDWR);
            }
        }
        g.connections.clear();
    }

    #[cfg(unix)]
    pub fn track_fd(&self, fd: std::os::fd::RawFd) {
        self.inner.lock().fds.insert(fd);
    }

    /// Register a live connection id (tests / custom dials).
    pub fn track(&self) -> usize {
        let mut g = self.inner.lock();
        let id = g.next_id;
        g.next_id = g.next_id.wrapping_add(1);
        g.connections.insert(id);
        id
    }

    pub fn forget(&self, id: usize) {
        self.inner.lock().connections.remove(&id);
    }

    /// Blocking dial used by tracker HTTP clients. Tracks the live socket so
    /// [`Self::reset`] can cut an old route promptly.
    pub fn dial_timeout(
        self: &Arc<Self>,
        address: &str,
        timeout: Duration,
    ) -> io::Result<TrackedConn> {
        let epoch_at_start = self.epoch();
        let addr: std::net::SocketAddr = address
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        let _ = stream.set_nodelay(true);
        if self.epoch() != epoch_at_start {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "network dialer reset during dial",
            ));
        }
        let id = self.track();
        Ok(TrackedConn {
            stream: Some(stream),
            id,
            dialer: Arc::clone(self),
        })
    }
}

/// TcpStream owned by a [`NetworkDialer`]; forgets itself on drop.
pub struct TrackedConn {
    stream: Option<TcpStream>,
    id: usize,
    dialer: Arc<NetworkDialer>,
}

impl TrackedConn {
    pub fn into_inner(mut self) -> TcpStream {
        self.dialer.forget(self.id);
        self.stream.take().expect("tracked conn stream")
    }
}

impl std::ops::Deref for TrackedConn {
    type Target = TcpStream;
    fn deref(&self) -> &TcpStream {
        self.stream.as_ref().expect("tracked conn stream")
    }
}

impl std::ops::DerefMut for TrackedConn {
    fn deref_mut(&mut self) -> &mut TcpStream {
        self.stream.as_mut().expect("tracked conn stream")
    }
}

impl Drop for TrackedConn {
    fn drop(&mut self) {
        self.dialer.forget(self.id);
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Generation gate for Android `ConnectivityManager` callbacks.
pub struct NetworkManager {
    generation: AtomicI64,
    known: AtomicBool,
    /// Monotonic counter of accepted (non-stale) changes; recovery callers
    /// can observe this to cancel a previous episode.
    episode: AtomicU64,
    dialer: Arc<NetworkDialer>,
}

impl Default for NetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkManager {
    pub fn new() -> Self {
        Self {
            generation: AtomicI64::new(-1),
            known: AtomicBool::new(false),
            episode: AtomicU64::new(0),
            dialer: Arc::new(NetworkDialer::new()),
        }
    }


    pub fn dialer(&self) -> Arc<NetworkDialer> {
        self.dialer.clone()
    }

    pub fn generation(&self) -> i64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn known(&self) -> bool {
        self.known.load(Ordering::SeqCst)
    }

    pub fn episode(&self) -> u64 {
        self.episode.load(Ordering::SeqCst)
    }

    /// Validate a `/network` JSON body the way Go does: `generation >= 0` and
    /// `connected` must be present.
    pub fn parse_change_body(body: &str) -> Result<(i64, bool), &'static str> {
        #[derive(serde::Deserialize)]
        struct Req {
            generation: i64,
            connected: Option<bool>,
        }
        let req: Req = serde_json::from_str(body).map_err(|_| "invalid network state")?;
        if req.generation < 0 {
            return Err("invalid network state");
        }
        let connected = req.connected.ok_or("invalid network state")?;
        Ok((req.generation, connected))
    }

    /// Apply a network-change event. Returns `Some(connected)` when the
    /// generation was accepted (caller should start/stop recovery). Returns
    /// `None` when the event was a duplicate/stale callback.
    pub fn on_network_change(&self, gen: i64, connected: bool) -> Option<bool> {
        if self.known.load(Ordering::SeqCst) && gen <= self.generation.load(Ordering::SeqCst) {
            return None;
        }
        self.known.store(true, Ordering::SeqCst);
        self.generation.store(gen, Ordering::SeqCst);
        self.episode.fetch_add(1, Ordering::SeqCst);
        self.dialer.reset();
        Some(connected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_and_duplicate_generations_are_ignored() {
        let n = NetworkManager::new();
        assert_eq!(n.on_network_change(1, true), Some(true));
        assert_eq!(n.generation(), 1);
        assert_eq!(n.on_network_change(1, false), None);
        assert_eq!(n.generation(), 1);
        assert_eq!(n.on_network_change(0, false), None);
        assert_eq!(n.on_network_change(2, true), Some(true));
        assert_eq!(n.generation(), 2);
        assert!(n.episode() >= 2);
    }

    #[test]
    fn parse_change_body_requires_connected() {
        assert!(NetworkManager::parse_change_body(r#"{"generation":1}"#).is_err());
        assert!(
            NetworkManager::parse_change_body(r#"{"generation":-1,"connected":true}"#).is_err()
        );
        assert_eq!(
            NetworkManager::parse_change_body(r#"{"generation":2,"connected":false}"#).unwrap(),
            (2, false)
        );
    }

    #[test]
    fn dialer_reset_bumps_epoch_and_clears_connections() {
        let d = NetworkDialer::new();
        let e0 = d.epoch();
        d.reset();
        assert_ne!(d.epoch(), e0);
        assert_eq!(d.connection_count(), 0);
    }
}

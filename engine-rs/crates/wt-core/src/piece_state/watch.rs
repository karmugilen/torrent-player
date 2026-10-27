//! Change versioning for `wait_change` (Go piece-state pubsub / version bump).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

/// Shared change counter + condvar. Torrents may use this or their own.
#[derive(Clone, Debug)]
pub struct ChangeWatch {
    version: Arc<AtomicU64>,
    lock: Arc<Mutex<()>>,
    cond: Arc<Condvar>,
}

impl ChangeWatch {
    pub fn new(initial: u64) -> Self {
        Self {
            version: Arc::new(AtomicU64::new(initial)),
            lock: Arc::new(Mutex::new(())),
            cond: Arc::new(Condvar::new()),
        }
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    pub fn bump(&self) -> u64 {
        let v = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        let _g = self.lock.lock();
        self.cond.notify_all();
        v
    }

    /// Block until `version() != after` or `timeout`. Returns current version.
    pub fn wait_change(&self, after: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        loop {
            let cur = self.version();
            if cur != after {
                return cur;
            }
            let now = Instant::now();
            if now >= deadline {
                return cur;
            }
            let mut g = self.lock.lock();
            let _ = self
                .cond
                .wait_for(&mut g, deadline.saturating_duration_since(now));
        }
    }
}

impl Default for ChangeWatch {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn bump_wakes_waiter() {
        let w = ChangeWatch::new(1);
        let w2 = w.clone();
        let h = thread::spawn(move || w2.wait_change(1, Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(20));
        w.bump();
        assert_ne!(h.join().unwrap(), 1);
    }
}

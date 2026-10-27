//! Half-open connection accounting and limits.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use wt_types::InfoHash;

/// Tracks in-flight half-open connections globally and per torrent.
#[derive(Clone, Default)]
pub struct HalfOpenTracker {
    inner: Arc<HalfOpenTrackerInner>,
}

#[derive(Default)]
struct HalfOpenTrackerInner {
    total: AtomicUsize,
    per_torrent: Mutex<HashMap<InfoHash, usize>>,
}

/// RAII guard releasing a global half-open slot when dropped.
pub struct GlobalHalfOpenGuard {
    tracker: Arc<HalfOpenTrackerInner>,
    disarmed: bool,
}

impl Drop for GlobalHalfOpenGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            self.tracker.total.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl GlobalHalfOpenGuard {
    /// Disarm the guard without decrementing (rarely needed).
    pub fn disarm(&mut self) {
        self.disarmed = true;
    }
}

/// RAII guard releasing a per-torrent half-open slot when dropped.
pub struct TorrentHalfOpenGuard {
    tracker: Arc<HalfOpenTrackerInner>,
    info_hash: InfoHash,
    disarmed: bool,
}

impl Drop for TorrentHalfOpenGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            let mut map = self.tracker.per_torrent.lock();
            if let Some(cnt) = map.get_mut(&self.info_hash) {
                if *cnt <= 1 {
                    map.remove(&self.info_hash);
                } else {
                    *cnt -= 1;
                }
            }
        }
    }
}

impl TorrentHalfOpenGuard {
    pub fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl HalfOpenTracker {
    /// Create a new tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to acquire a slot against `total_half_open_conns`.
    pub fn acquire_global(&self, max_total: usize) -> Option<GlobalHalfOpenGuard> {
        let mut curr = self.inner.total.load(Ordering::Relaxed);
        loop {
            if curr >= max_total {
                return None;
            }
            match self.inner.total.compare_exchange_weak(
                curr,
                curr + 1,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(GlobalHalfOpenGuard {
                        tracker: self.inner.clone(),
                        disarmed: false,
                    })
                }
                Err(actual) => curr = actual,
            }
        }
    }

    /// Try to acquire a slot for a specific torrent.
    pub fn acquire_torrent(
        &self,
        info_hash: InfoHash,
        max_per_torrent: usize,
    ) -> Option<TorrentHalfOpenGuard> {
        let mut map = self.inner.per_torrent.lock();
        let cnt = map.entry(info_hash).or_insert(0);
        if *cnt >= max_per_torrent {
            None
        } else {
            *cnt += 1;
            Some(TorrentHalfOpenGuard {
                tracker: self.inner.clone(),
                info_hash,
                disarmed: false,
            })
        }
    }

    /// Current global half-open count.
    pub fn current_total(&self) -> usize {
        self.inner.total.load(Ordering::Relaxed)
    }

    /// Current half-open count for a specific torrent.
    pub fn current_for_torrent(&self, info_hash: &InfoHash) -> usize {
        self.inner
            .per_torrent
            .lock()
            .get(info_hash)
            .copied()
            .unwrap_or(0)
    }
}

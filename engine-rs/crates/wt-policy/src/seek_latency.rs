//! Seek latency stats and Claude-seek helpers (`engine-go/claude_seek.go`).
//!
//! Record seek-to-first-byte times with [`SeekLatencyStats`]. Tuning constants
//! and the drop-chokers decision are pure so the engine can apply them to
//! wt-core without sleeping in tests.

use std::time::Duration;

use crate::constants::{
    CHOKE_TIMEOUT, CLAUDE_SEEK_NUDGE_EVERY, CLAUDE_SEEK_STEAL_AFTER, SEEK_STATS_WINDOW,
};
use crate::peer_exploration::PeerKey;

/// Snapshot exported on `/stats` / settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SeekStats {
    pub count: i32,
    #[serde(rename = "lastMs")]
    pub last_ms: i64,
    #[serde(rename = "medianMs")]
    pub median_ms: i64,
    pub stalls: i32,
}

/// Rolling seek-to-first-byte window since last reset.
#[derive(Clone, Debug, Default)]
pub struct SeekLatencyStats {
    recent: Vec<Duration>,
    count: i32,
    stalls: i32,
}

impl SeekLatencyStats {
    pub fn record_seek(&mut self, d: Duration) {
        self.count += 1;
        self.recent.push(d);
        if self.recent.len() > SEEK_STATS_WINDOW {
            let skip = self.recent.len() - SEEK_STATS_WINDOW;
            self.recent.drain(..skip);
        }
    }

    pub fn record_stall(&mut self) {
        self.stalls += 1;
    }

    pub fn reset(&mut self) {
        self.recent.clear();
        self.count = 0;
        self.stalls = 0;
    }

    pub fn snapshot(&self) -> SeekStats {
        let mut out = SeekStats {
            count: self.count,
            stalls: self.stalls,
            ..Default::default()
        };
        if let Some(last) = self.recent.last() {
            out.last_ms = last.as_millis() as i64;
            let mut sorted = self.recent.clone();
            sorted.sort();
            out.median_ms = sorted[sorted.len() / 2].as_millis() as i64;
        }
        out
    }
}

/// Shipped Claude-seek timings (variables in Go so seek-search can retune).
#[derive(Clone, Copy, Debug)]
pub struct ClaudeSeekTuning {
    pub steal_after: Duration,
    pub nudge_every: Duration,
}

impl Default for ClaudeSeekTuning {
    fn default() -> Self {
        Self {
            steal_after: CLAUDE_SEEK_STEAL_AFTER,
            nudge_every: CLAUDE_SEEK_NUDGE_EVERY,
        }
    }
}

/// Stalled-read retry schedule used by playback readers.
pub fn claude_seek_read_attempts() -> &'static [Duration] {
    const AT: &[Duration] = &[
        Duration::from_secs(3),
        Duration::from_secs(6),
        Duration::from_secs(12),
        Duration::from_secs(24),
    ];
    AT
}

/// Steal-after duration to push into wt-core when Claude seek toggles.
pub fn urgent_steal_after(enabled: bool, tuning: &ClaudeSeekTuning) -> Duration {
    if enabled {
        tuning.steal_after
    } else {
        Duration::ZERO
    }
}

/// Peer choked while we want its data.
#[derive(Clone, Debug)]
pub struct ChokedPeer {
    pub peer: PeerKey,
    pub choked_for: Duration,
    pub we_want_data: bool,
}

/// Peers that should be dropped this tick (WebTorrent CHOKE_TIMEOUT).
pub fn drop_chokers(enabled: bool, peers: &[ChokedPeer]) -> Vec<PeerKey> {
    if !enabled {
        return Vec::new();
    }
    peers
        .iter()
        .filter(|p| p.we_want_data && p.choked_for >= CHOKE_TIMEOUT)
        .map(|p| p.peer)
        .collect()
}

pub fn choke_timeout() -> Duration {
    CHOKE_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_stats_median_and_window() {
        let mut s = SeekLatencyStats::default();
        for ms in [10u64, 30, 20] {
            s.record_seek(Duration::from_millis(ms));
        }
        let snap = s.snapshot();
        assert_eq!(snap.count, 3);
        assert_eq!(snap.last_ms, 20);
        assert_eq!(snap.median_ms, 20);
        for _ in 0..SEEK_STATS_WINDOW + 5 {
            s.record_seek(Duration::from_millis(5));
        }
        assert!(s.snapshot().count > SEEK_STATS_WINDOW as i32);
        assert_eq!(s.recent.len(), SEEK_STATS_WINDOW);
        s.record_stall();
        assert_eq!(s.snapshot().stalls, 1);
        s.reset();
        assert_eq!(s.snapshot(), SeekStats::default());
    }

    #[test]
    fn drop_chokers_respects_timeout_and_flag() {
        let peers = [
            ChokedPeer {
                peer: 1,
                choked_for: Duration::from_secs(6),
                we_want_data: true,
            },
            ChokedPeer {
                peer: 2,
                choked_for: Duration::from_secs(1),
                we_want_data: true,
            },
            ChokedPeer {
                peer: 3,
                choked_for: Duration::from_secs(10),
                we_want_data: false,
            },
        ];
        assert!(drop_chokers(false, &peers).is_empty());
        assert_eq!(drop_chokers(true, &peers), vec![1]);
    }
}

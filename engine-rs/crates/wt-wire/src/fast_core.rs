//! Keep the uTP receive thread on the phone's fast cores during playback.
//!
//! Only while a stream is open: otherwise Android's own placement saves battery.
use std::sync::atomic::{AtomicUsize, Ordering};

static PLAYBACK_STREAMS: AtomicUsize = AtomicUsize::new(0);

pub fn playback_streams() -> usize {
    PLAYBACK_STREAMS.load(Ordering::Relaxed)
}

/// Held by an open playback stream.
pub struct PlaybackHold(());

impl PlaybackHold {
    pub fn new() -> Self {
        PLAYBACK_STREAMS.fetch_add(1, Ordering::Relaxed);
        Self(())
    }
}

impl Default for PlaybackHold {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PlaybackHold {
    fn drop(&mut self) {
        PLAYBACK_STREAMS.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) use linux::CorePin;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod linux {
    use super::{Ordering, PLAYBACK_STREAMS};
    use nix::sched::{sched_getaffinity, sched_setaffinity, CpuSet};
    use nix::unistd::Pid;

    /// Affinity of the calling thread, which must be the one that made it.
    pub(crate) struct CorePin {
        /// The thread's own cores, and the fastest of them. `None` when there
        /// is nothing to choose: all cores alike, or unreadable.
        cores: Option<(CpuSet, CpuSet)>,
        pinned: bool,
    }

    impl CorePin {
        pub(crate) fn new() -> Self {
            Self { cores: cores(), pinned: false }
        }

        /// Follow playback. A refused change (Android limits background apps
        /// to some cores) leaves things as they are, and is not retried.
        pub(crate) fn follow_playback(&mut self) {
            let playing = PLAYBACK_STREAMS.load(Ordering::Relaxed) > 0;
            if playing == self.pinned {
                return;
            }
            self.pinned = playing;
            if let Some((all, fast)) = &self.cores {
                let _ = sched_setaffinity(Pid::from_raw(0), if playing { fast } else { all });
            }
        }
    }

    fn cores() -> Option<(CpuSet, CpuSet)> {
        let all = sched_getaffinity(Pid::from_raw(0)).ok()?;
        let speeds: Vec<(usize, u64)> = (0..CpuSet::count())
            .filter(|&cpu| all.is_set(cpu).unwrap_or(false))
            .filter_map(|cpu| Some((cpu, max_freq(cpu)?)))
            .collect();
        let top = speeds.iter().map(|&(_, speed)| speed).max()?;
        if speeds.iter().all(|&(_, speed)| speed == top) {
            return None;
        }
        let mut fast = CpuSet::new();
        for &(cpu, speed) in &speeds {
            if speed == top {
                fast.set(cpu).ok()?;
            }
        }
        Some((all, fast))
    }

    fn max_freq(cpu: usize) -> Option<u64> {
        std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) struct CorePin;

#[cfg(not(any(target_os = "linux", target_os = "android")))]
impl CorePin {
    pub(crate) fn new() -> Self {
        Self
    }
    pub(crate) fn follow_playback(&mut self) {}
}

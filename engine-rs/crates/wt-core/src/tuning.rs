use std::sync::atomic::{AtomicBool, Ordering};

/// The user's playback switches. Everything else about scheduling is fixed in
/// [`crate::sched`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeekTuning {
    /// Send second copies of late playhead blocks.
    pub hedge: bool,
    /// Slow peers leave `Next` pieces to faster ones.
    pub slow_peer_yield: bool,
}

static HEDGE: AtomicBool = AtomicBool::new(false);
static SLOW_PEER_YIELD: AtomicBool = AtomicBool::new(false);

pub fn set_seek_tuning(tuning: SeekTuning) {
    HEDGE.store(tuning.hedge, Ordering::Relaxed);
    SLOW_PEER_YIELD.store(tuning.slow_peer_yield, Ordering::Relaxed);
}

pub fn current_seek_tuning() -> SeekTuning {
    SeekTuning {
        hedge: HEDGE.load(Ordering::Relaxed),
        slow_peer_yield: SLOW_PEER_YIELD.load(Ordering::Relaxed),
    }
}

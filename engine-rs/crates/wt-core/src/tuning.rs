use parking_lot::RwLock;
use std::sync::LazyLock;

/// Webtor seek rules (`torrent.SeekTuning` in webtor_urgent.go).
#[derive(Clone, Debug, PartialEq)]
pub struct SeekTuning {
    pub focus_blocks: i32,
    pub focus_steal_divisor: i32,
    pub faster_ratio: f64,
    pub queue_aware: bool,
    pub bound_candidates: bool,
    pub pipeline_seconds: f64,
    pub min_pipeline: i32,
    pub full_pipeline: bool,
    pub pipeline_rtt: f64,
    pub arrival_aware: bool,
    pub arrival_ratio: f64,
    pub no_balance_steal: bool,
    pub refill_at: f64,
    pub hol_rescue: bool,
    pub hol_after_ms: f64,
    pub hedge: bool,
    pub slow_peer_yield: bool,
    pub learned_arrival: bool,
    pub peak_mode: PeakMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PeakMode {
    #[default]
    Added,
    WholeQueue,
    KeepOnTopUp,
}

impl Default for SeekTuning {
    fn default() -> Self {
        SeekTuning {
            focus_blocks: 8,
            focus_steal_divisor: 2,
            faster_ratio: 3.0,
            queue_aware: false,
            bound_candidates: true,
            pipeline_seconds: 0.0,
            min_pipeline: 0,
            full_pipeline: false,
            pipeline_rtt: 2.0,
            arrival_aware: true,
            arrival_ratio: 1.5,
            no_balance_steal: false,
            refill_at: 0.0,
            hol_rescue: false,
            hol_after_ms: 300.0,
            hedge: false,
            slow_peer_yield: false,
            learned_arrival: false,
            peak_mode: PeakMode::WholeQueue,
        }
    }
}

static CURRENT_TUNING: LazyLock<RwLock<SeekTuning>> =
    LazyLock::new(|| RwLock::new(SeekTuning::default()));

pub fn set_seek_tuning(mut tuning: SeekTuning) {
    if tuning.focus_blocks < 1 {
        tuning.focus_blocks = 1;
    }
    if tuning.focus_steal_divisor < 1 {
        tuning.focus_steal_divisor = 1;
    }
    *CURRENT_TUNING.write() = tuning;
}

pub fn current_seek_tuning() -> SeekTuning {
    CURRENT_TUNING.read().clone()
}

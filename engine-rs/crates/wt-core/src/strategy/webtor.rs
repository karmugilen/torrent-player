//! Webtor request-selection hooks from `webtor_urgent.go`.
//!
//! Only the pure bits that feed desired-request computation: candidate budget,
//! focus distance, and playback pipeline limit. Steal/HOL/hedge stay in the
//! peer/torrent layer (agy-2).

/// Default `MinPipeline` floor when PipelineRTT sizing is active (Go uses
/// `max(tuning.MinPipeline, 2)`).
pub const DEFAULT_MIN_PIPELINE: u32 = 2;

/// Selection-relevant subset of `SeekTuning`.
#[derive(Clone, Copy, Debug)]
pub struct RequestTuning {
    pub bound_candidates: bool,
    pub pipeline_seconds: f64,
    pub min_pipeline: u32,
    pub pipeline_rtt: f64,
    /// Playback is active (request focus set). Pipeline caps only apply then.
    pub playback_active: bool,
}

impl Default for RequestTuning {
    fn default() -> Self {
        Self {
            bound_candidates: true,
            pipeline_seconds: 0.0,
            min_pipeline: 0,
            pipeline_rtt: 2.0,
            playback_active: false,
        }
    }
}

impl RequestTuning {
    /// Build from wt-core `SeekTuning` plus whether a request focus is set.
    pub fn from_seek_tuning(
        bound_candidates: bool,
        pipeline_seconds: f64,
        min_pipeline: i32,
        pipeline_rtt: f64,
        playback_active: bool,
    ) -> Self {
        Self {
            bound_candidates,
            pipeline_seconds,
            min_pipeline: min_pipeline.max(0) as u32,
            pipeline_rtt,
            playback_active,
        }
    }
}

/// How many free (usable) blocks a peer collects before it stops scanning
/// lower-priority pieces, or `0` for no limit.
///
/// Go: `max(2 * nominalMaxRequests(), 256)` when `BoundCandidates`.
pub fn candidate_budget(nominal_max_requests: u32, bound_candidates: bool) -> usize {
    if !bound_candidates {
        return 0;
    }
    (2 * nominal_max_requests as usize).max(256)
}

/// Orders pieces from the focus piece forward, then those behind it, nearest
/// first. Matches Go `focusDistance`.
pub fn focus_distance(piece: u32, focus: u32) -> i32 {
    if piece >= focus {
        (piece - focus) as i32
    } else {
        // math.MaxInt32/2 + (focus - piece)
        (i32::MAX / 2).saturating_add((focus - piece) as i32)
    }
}

/// Max outstanding requests for a peer during playback, or `None` for uncapped.
///
/// Prefers PipelineRTT (BDP) when `pipeline_rtt > 0`, else `pipeline_seconds`.
pub fn playback_pipeline_limit(
    tuning: &RequestTuning,
    download_rate_bps: f64,
    rtt_secs: f64,
    chunk_size: u32,
) -> Option<u32> {
    if !tuning.playback_active || chunk_size == 0 {
        return None;
    }
    if tuning.pipeline_rtt > 0.0 {
        if rtt_secs <= 0.0 || download_rate_bps <= 0.0 {
            return None;
        }
        let min_pipe = tuning.min_pipeline.max(DEFAULT_MIN_PIPELINE);
        let n = (download_rate_bps * rtt_secs * tuning.pipeline_rtt / f64::from(chunk_size)) as u32
            + min_pipe;
        return Some(n);
    }
    if tuning.pipeline_seconds <= 0.0 {
        return None;
    }
    let n = (download_rate_bps * tuning.pipeline_seconds / f64::from(chunk_size)) as u32 + 1;
    Some(n.max(tuning.min_pipeline).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_budget_matches_go() {
        assert_eq!(candidate_budget(10, false), 0);
        assert_eq!(candidate_budget(10, true), 256); // 2*10=20 < 256
        assert_eq!(candidate_budget(200, true), 400);
    }

    #[test]
    fn focus_distance_forward_then_behind() {
        assert_eq!(focus_distance(10, 10), 0);
        assert_eq!(focus_distance(12, 10), 2);
        assert!(focus_distance(9, 10) > focus_distance(100, 10));
        assert!(focus_distance(8, 10) > focus_distance(9, 10));
    }

    #[test]
    fn pipeline_rtt_sizing() {
        let tuning = RequestTuning {
            bound_candidates: true,
            pipeline_seconds: 0.0,
            min_pipeline: 0,
            pipeline_rtt: 2.0,
            playback_active: true,
        };
        // 160 KiB/s, 100ms RTT, 16 KiB chunks → 160000*0.1*2/16384 + 2 ≈ 1.95+2 → 3
        let lim = playback_pipeline_limit(&tuning, 160_000.0, 0.1, 16_384).unwrap();
        assert_eq!(lim, 3);
    }
}

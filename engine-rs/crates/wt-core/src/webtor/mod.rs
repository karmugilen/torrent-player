//! Webtor seek runtime decisions (`engine-go/patches/torrent/webtor_urgent.go`).
//!
//! Pure decision functions + small state (`RttEstimator`, counters) over
//! [`PeerView`] / [`TorrentView`] / [`Clock`] / [`ArrivalPredictor`]. agy-2
//! calls these from the peer and request loops; selection helpers already in
//! [`crate::strategy::webtor`] (candidate budget, focus distance, pipeline
//! limit) are reused and not duplicated here.
//!
//! # Call sites (Go → when → Rust)
//!
//! | Go | When | Rust API |
//! |---|---|---|
//! | `Client.SetUrgentRequestStealing` | engine config | store `Duration`; pass into steal APIs |
//! | `PeerConn.shouldStealUrgent` | `requesting.go` steal gate | [`should_steal_urgent`] |
//! | `requesting.go` NoBalanceSteal branch | taking a block another peer holds | [`may_take_existing_request`] |
//! | `PeerConn.fasterFocusPeer` | desired-request fill of playhead block | [`faster_focus_peer`] |
//! | `PeerConn.expectedArrival` | fasterFocus / HOL / hedge / refocus sort | [`expected_arrival_secs`] / [`expected_arrival_with_learned`] |
//! | `rttEstimator.sent/received` | request send / chunk receive | [`RttEstimator`] |
//! | `PeerConn.pendingRequests` | nudge / fasterFocus / pipeline | [`pending_requests`] |
//! | `PeerConn.urgentPastCancels` | refill past cancel backlog | [`urgent_past_cancels`] |
//! | `Torrent.requestFocusIndex` / `SetRequestFocus` | seek / reader | [`request_focus_index`] / [`encode_request_focus`] |
//! | `Torrent.RefocusRequests` | after seek | [`refocus_should_cancel`] + [`refocus_sort_key`] then refill |
//! | `Torrent.NudgeUrgentRequests` | reader wait (~100 ms) | [`should_nudge_peer`] after HOL/hedge |
//! | `webtorHeadOfLine` / `webtorRescueHeadOfLine` | start of nudge | [`head_of_line`] / [`rescue_decisions`] |
//! | `webtorHedgeHeadOfLine` / `webtorSendHedge` / `webtorHedgeAnswered` | start of nudge / chunk path | [`hedge_decisions`] / [`hedge_answered`] |
//! | `webtorYieldToFasterPeers` | `requesting.go` piece walk | [`yield_to_faster_peers`] |
//! | `WebtorDropChokers` | periodic | [`chokers_to_drop`] |
//! | `webtorHasPlayingPieceWeLack` | interest calc | [`has_playing_piece_we_lack`] |
//! | `WebtorPeerCounts` | diagnostics | [`peer_counts`] |
//! | PeakMode on request update | `updateRequests` | [`next_peak_requests`] / [`nominal_from_peak`] |
//! | LearnedArrival | expectedArrival / overdue | [`ArrivalPredictor`] via agy-2 `arrival` |
//!
//! # Not here
//!
//! - `candidate_budget` / `focus_distance` / `playback_pipeline_limit` →
//!   [`crate::strategy::webtor`]
//! - Hashing order for Now pieces → grok-6 `hashing/`
//! - Wire I/O, locks, actual cancel/request sends → agy-2 peer/torrent loops

#![allow(clippy::too_many_arguments)] // mirrors Go method arity

pub mod clock;
pub mod decisions;
pub mod hol;
pub mod rtt;
pub mod traits;

pub use clock::{age, Clock, ManualClock, SystemClock};
pub use decisions::{
    chokers_to_drop, encode_request_focus, expected_arrival_secs, expected_arrival_with_learned,
    faster_focus_peer, has_playing_piece_we_lack, may_take_existing_request, next_peak_requests,
    nominal_from_peak, peer_counts, pending_requests, refocus_should_cancel, refocus_sort_key,
    request_focus_index, should_nudge_peer, should_steal_urgent, steal_lands_in_flight,
    urgent_past_cancels,
    yield_to_faster_peers, DEFAULT_RTT_SECS, SLOW_PEER_BYTES_PER_SEC, SLOW_PEER_YIELDS_PER_REFILL,
};
pub use hol::{
    block_is_overdue, head_of_line, hedge_answered, hedge_decisions, rescue_decisions, HedgeMove,
    RescueMove, HEDGES_PER_NUDGE, HOL_RESCUES_PER_NUDGE, HOL_WINDOW_BLOCKS,
};
pub use rtt::RttEstimator;
pub use traits::{ArrivalPredictor, PeerView, TorrentView};

use std::sync::atomic::{AtomicI64, Ordering};

/// Counters mirroring Go `webtorRescues` / `webtorHedgeSent` / `webtorChokersDropped`.
#[derive(Debug, Default)]
pub struct WebtorCounters {
    pub rescues: AtomicI64,
    pub hedges: AtomicI64,
    pub chokers_dropped: AtomicI64,
}

impl WebtorCounters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_rescues(&self, n: i64) {
        self.rescues.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_hedges(&self, n: i64) {
        self.hedges.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_chokers_dropped(&self, n: i64) {
        self.chokers_dropped.fetch_add(n, Ordering::Relaxed);
    }

    pub fn rescues(&self) -> i64 {
        self.rescues.load(Ordering::Relaxed)
    }

    pub fn hedges(&self) -> i64 {
        self.hedges.load(Ordering::Relaxed)
    }

    pub fn chokers_dropped(&self) -> i64 {
        self.chokers_dropped.load(Ordering::Relaxed)
    }
}

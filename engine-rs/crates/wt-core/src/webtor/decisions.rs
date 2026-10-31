//! Pure seek-decision helpers ported from `webtor_urgent.go` / `requesting.go`.

#![allow(clippy::too_many_arguments)]

use std::time::Duration;

use wt_types::{PieceIndex, PiecePriority, RequestIndex};

use crate::tuning::{PeakMode, SeekTuning};

use super::clock::{age, Clock};
use super::traits::{ArrivalPredictor, PeerView};

/// Default RTT assumption when none measured yet (Go `expectedArrival`).
pub const DEFAULT_RTT_SECS: f64 = 0.2;

/// Three blocks/s — WebTorrent `SPEED_THRESHOLD` (`slowPeerBytesPerSec`).
pub const SLOW_PEER_BYTES_PER_SEC: f64 = (3 * 16 * 1024) as f64;

/// Bounds pieces one refill yields (`slowPeerYieldsPerRefill`).
pub const SLOW_PEER_YIELDS_PER_REFILL: u32 = 10;

use std::time::Instant;

/// Convert playhead byte offset to request index.
///
/// Go `requestFocusIndex`: `off = store-1`; negative clears. Piece begin +
/// offset within piece / chunkSize. Assumes uniform piece length.
pub fn request_focus_index(
    stored_off_plus_one: i64,
    have_info: bool,
    total_length: i64,
    piece_length: i64,
    chunk_size: u32,
) -> Option<RequestIndex> {
    let off = stored_off_plus_one - 1;
    if off < 0 || !have_info || off >= total_length || piece_length <= 0 || chunk_size == 0 {
        return None;
    }
    let piece = (off / piece_length) as u32;
    let within = (off % piece_length) as u32;
    let chunks_per = (piece_length as u32) / chunk_size;
    Some(piece * chunks_per + within / chunk_size)
}

/// Encode focus offset the way Go `SetRequestFocus` does: `store(max(off+1, 0))`.
pub fn encode_request_focus(off: i64) -> i64 {
    (off + 1).max(0)
}

/// Go `shouldStealUrgent`.
pub fn should_steal_urgent(
    clock: &dyn Clock,
    tuning: &SeekTuning,
    urgent_steal_after: Duration,
    req: RequestIndex,
    piece_priority: PiecePriority,
    request_when: Instant,
    stealer_last_useful: Option<Instant>,
    holder_last_useful: Option<Instant>,
    focus: Option<RequestIndex>,
) -> bool {
    if urgent_steal_after.is_zero() {
        return false;
    }
    if piece_priority < PiecePriority::Now {
        return false;
    }
    let mut after = urgent_steal_after;
    if let Some(f) = focus {
        if req >= f && req < f + tuning.focus_blocks as u32 {
            after /= tuning.focus_steal_divisor.max(1) as u32;
        }
    }
    if age(clock, request_when) < after {
        return false;
    }
    let Some(stealer_useful) = stealer_last_useful else {
        return false;
    };
    if age(clock, stealer_useful) > after {
        return false;
    }
    match holder_last_useful {
        Some(h) => stealer_useful > h,
        None => true,
    }
}

/// Whether a holder has most likely sent a block already, given how many of
/// its requests were queued ahead of it. A peer serves its queue in order;
/// by the time a cancel reaches it, roughly one bandwidth-delay product of
/// blocks plus the one on the wire has left. Stealing those only duplicates
/// them.
pub fn steal_lands_in_flight(ahead: usize, holder_rate_bps: f64, holder_rtt: Duration, chunk: u32) -> bool {
    let bdp = holder_rate_bps.max(0.0) * holder_rtt.as_secs_f64() / f64::from(chunk.max(1));
    (ahead as f64) < bdp.ceil() + 1.0
}

/// Go `pendingRequests`: requests (+ cancelled unless FullPipeline).
pub fn pending_requests(tuning: &SeekTuning, outstanding: u32, cancelled: u32) -> u32 {
    if tuning.full_pipeline {
        outstanding
    } else {
        outstanding.saturating_add(cancelled)
    }
}

/// Go `urgentPastCancels`.
pub fn urgent_past_cancels(
    outstanding: u32,
    nominal_max: u32,
    piece_priority: PiecePriority,
) -> bool {
    if outstanding >= nominal_max {
        return false;
    }
    piece_priority >= PiecePriority::Now
}

/// Go `expectedArrival` without learned model: RTT + (queued+1)*chunk/rate.
/// Unmeasured RTT → 200 ms. Rate ≤ 0 → +∞.
pub fn expected_arrival_secs(
    download_rate_bps: f64,
    rtt: Duration,
    queued_requests: u32,
    chunk_size: u32,
) -> f64 {
    if download_rate_bps <= 0.0 {
        return f64::MAX;
    }
    let mut rtt_s = rtt.as_secs_f64();
    if rtt_s <= 0.0 {
        rtt_s = DEFAULT_RTT_SECS;
    }
    let queued = f64::from(queued_requests + 1);
    rtt_s + queued * f64::from(chunk_size) / download_rate_bps
}

/// Learned path when `SeekTuning.learned_arrival` and predictor + features ok.
pub fn expected_arrival_with_learned(
    tuning: &SeekTuning,
    predictor: Option<&dyn ArrivalPredictor>,
    features: Option<[f64; crate::arrival::ARRIVAL_FEATURES]>,
    download_rate_bps: f64,
    rtt: Duration,
    queued_requests: u32,
    chunk_size: u32,
) -> f64 {
    if tuning.learned_arrival {
        if let (Some(p), Some(x)) = (predictor, features) {
            return p.predict(x);
        }
    }
    expected_arrival_secs(download_rate_bps, rtt, queued_requests, chunk_size)
}

fn peer_score(tuning: &SeekTuning, rate: f64, queued: u32, chunk_size: u32) -> f64 {
    if !tuning.queue_aware || rate <= 0.0 {
        return rate;
    }
    let q = f64::from(queued + 1) * f64::from(chunk_size);
    rate / q
}

/// Go `fasterFocusPeer` decision: pick the best other peer that should take
/// playhead block `req` from `self_peer`, or `None`.
#[allow(clippy::needless_lifetimes)]
pub fn faster_focus_peer<'a, P: PeerView>(
    tuning: &SeekTuning,
    urgent_steal_after: Duration,
    self_peer: &P,
    candidates: &'a [P],
    req: RequestIndex,
    piece: PieceIndex,
    piece_priority: PiecePriority,
    focus: Option<RequestIndex>,
    chunk_size: u32,
    learned: Option<&dyn ArrivalPredictor>,
    features_for: &dyn Fn(&P) -> Option<[f64; crate::arrival::ARRIVAL_FEATURES]>,
) -> Option<&'a P>
where
    P::Id: PartialEq,
{
    if urgent_steal_after.is_zero() || tuning.faster_ratio <= 0.0 {
        return None;
    }
    let focus = focus?;
    if req < focus || req >= focus + tuning.focus_blocks as u32 {
        return None;
    }
    if piece_priority < PiecePriority::Now {
        return None;
    }

    if tuning.arrival_aware {
        let ratio = tuning.arrival_ratio.max(1.0);
        let self_at = expected_arrival_with_learned(
            tuning,
            learned,
            features_for(self_peer),
            self_peer.download_rate_bps(),
            self_peer.rtt(),
            self_peer.queued_requests(),
            chunk_size,
        ) / ratio;
        let mut best: Option<&P> = None;
        let mut best_at = self_at;
        for c in candidates {
            if c.id() == self_peer.id() || c.closed() || c.peer_choking() || !c.has_piece(piece) {
                continue;
            }
            if pending_requests(tuning, c.queued_requests(), c.cancelled_requests())
                >= c.nominal_max_requests()
            {
                continue;
            }
            let at = expected_arrival_with_learned(
                tuning,
                learned,
                features_for(c),
                c.download_rate_bps(),
                c.rtt(),
                c.queued_requests(),
                chunk_size,
            );
            if at < best_at {
                best = Some(c);
                best_at = at;
            }
        }
        return best;
    }

    let self_score = peer_score(
        tuning,
        self_peer.download_rate_bps(),
        self_peer.queued_requests(),
        chunk_size,
    );
    let mut best: Option<&P> = None;
    let mut best_score = tuning.faster_ratio * self_score;
    for c in candidates {
        if c.id() == self_peer.id() || c.closed() || c.peer_choking() || !c.has_piece(piece) {
            continue;
        }
        if pending_requests(tuning, c.queued_requests(), c.cancelled_requests())
            >= c.nominal_max_requests()
        {
            continue;
        }
        let s = peer_score(
            tuning,
            c.download_rate_bps(),
            c.queued_requests(),
            chunk_size,
        );
        if s > best_score {
            best = Some(c);
            best_score = s;
        }
    }
    best
}

/// Whether the balance / NoBalanceSteal gate allows taking `req` from
/// `existing` (requesting.go one-condition change).
///
/// Returns true if the stealer may cancel+take.
pub fn may_take_existing_request(
    clock: &dyn Clock,
    tuning: &SeekTuning,
    focus_set: bool,
    urgent_steal_after: Duration,
    req: RequestIndex,
    piece_priority: PiecePriority,
    request_when: Instant,
    stealer_queued: u32,
    stealer_last_useful: Option<Instant>,
    holder_uncancelled: u32,
    holder_last_useful: Option<Instant>,
    focus: Option<RequestIndex>,
    triggered_by_cancel: bool,
) -> bool {
    if triggered_by_cancel {
        return false;
    }
    if tuning.no_balance_steal && focus_set {
        return should_steal_urgent(
            clock,
            tuning,
            urgent_steal_after,
            req,
            piece_priority,
            request_when,
            stealer_last_useful,
            holder_last_useful,
            focus,
        );
    }
    // Upstream balance: steal if we end with ≤1 more than holder, preferring
    // more recent useful data; urgent steal overrides.
    let diff = i64::from(stealer_queued) + 1 - (i64::from(holder_uncancelled) - 1);
    let balance_blocks = diff > 1
        || (diff == 1
            && !stealer_last_useful
                .zip(holder_last_useful)
                .is_some_and(|(s, h)| s > h));
    if balance_blocks
        && !should_steal_urgent(
            clock,
            tuning,
            urgent_steal_after,
            req,
            piece_priority,
            request_when,
            stealer_last_useful,
            holder_last_useful,
            focus,
        )
    {
        return false;
    }
    true
}

/// Go `webtorYieldToFasterPeers`.
pub fn yield_to_faster_peers<P: PeerView>(
    self_peer: &P,
    candidates: &[P],
    piece: PieceIndex,
    piece_bytes_left: u64,
    chunk_size: u32,
) -> bool
where
    P::Id: PartialEq,
{
    let mut speed = self_peer.download_rate_bps();
    if speed >= SLOW_PEER_BYTES_PER_SEC {
        return false;
    }
    speed = speed.max(1.0);
    let secs = f64::from(self_peer.queued_requests().max(1)) * f64::from(chunk_size) / speed;
    let mut missing = piece_bytes_left as f64;
    for c in candidates {
        if c.id() == self_peer.id() || c.closed() || c.peer_choking() || !c.has_piece(piece) {
            continue;
        }
        let other = c.download_rate_bps();
        if other < SLOW_PEER_BYTES_PER_SEC || other <= speed {
            continue;
        }
        missing -= other * secs;
        if missing <= 0.0 {
            return true;
        }
    }
    false
}

/// Go `webtorHasPlayingPieceWeLack`.
pub fn has_playing_piece_we_lack<P: PeerView>(
    _peer: &P,
    interest: Option<(PieceIndex, PieceIndex)>,
    data_download_disallowed: bool,
    peer_has_all: Option<bool>,
    peer_pieces: impl Iterator<Item = PieceIndex>,
    mut we_lack: impl FnMut(PieceIndex) -> bool,
    num_pieces: PieceIndex,
) -> bool {
    let Some((begin, end)) = interest else {
        return false;
    };
    if begin >= end || data_download_disallowed {
        return false;
    }
    if let Some(true) = peer_has_all {
        let end = end.min(num_pieces);
        for i in begin..end {
            if we_lack(i) {
                return true;
            }
        }
        return false;
    }
    for i in peer_pieces {
        if i < begin {
            continue;
        }
        if i >= end {
            return false;
        }
        if we_lack(i) {
            return true;
        }
    }
    false
}

/// Sort key for refocus: lower is sooner / should refill first.
pub fn refocus_sort_key<P: PeerView>(
    tuning: &SeekTuning,
    peer: &P,
    chunk_size: u32,
    learned: Option<&dyn ArrivalPredictor>,
    features: Option<[f64; crate::arrival::ARRIVAL_FEATURES]>,
) -> f64 {
    if tuning.arrival_aware {
        expected_arrival_with_learned(
            tuning,
            learned,
            features,
            peer.download_rate_bps(),
            peer.rtt(),
            peer.queued_requests(),
            chunk_size,
        )
    } else {
        // Sort by descending rate → use negative rate as ascending key.
        -peer.download_rate_bps()
    }
}

/// Whether a non-Now request should be cancelled on refocus.
pub fn refocus_should_cancel(piece_priority: PiecePriority) -> bool {
    piece_priority < PiecePriority::Now
}

/// Peak after a request update (Go PeakMode).
pub fn next_peak_requests(
    mode: PeakMode,
    previous_peak: u32,
    outstanding_after: u32,
    added_this_update: u32,
    was_empty_before: bool,
) -> u32 {
    match mode {
        PeakMode::Added => added_this_update,
        PeakMode::WholeQueue => outstanding_after,
        PeakMode::KeepOnTopUp => {
            if was_empty_before {
                outstanding_after.max(added_this_update)
            } else {
                previous_peak
            }
        }
    }
}

/// Nominal max from peak (Go uses twice peak, floored at 1).
pub fn nominal_from_peak(peak: u32) -> u32 {
    (peak * 2).max(1)
}

/// Whether a peer should be nudged for urgent pieces (has spare slots + piece).
pub fn should_nudge_peer<P: PeerView>(
    tuning: &SeekTuning,
    peer: &P,
    urgent_pieces: &[PieceIndex],
) -> bool {
    if peer.closed() {
        return false;
    }
    let pending = pending_requests(tuning, peer.queued_requests(), peer.cancelled_requests());
    if pending >= peer.nominal_max_requests() {
        return false;
    }
    for &i in urgent_pieces {
        if !peer.has_piece(i) {
            continue;
        }
        if peer.peer_choking() && !peer.allowed_fast(i) {
            continue;
        }
        return true;
    }
    false
}

/// Chokers eligible to drop (Go `WebtorDropChokers` selection, at most 2).
pub fn chokers_to_drop<'a, P: PeerView>(
    clock: &dyn Clock,
    peers: &'a [P],
    after: Duration,
    waiting_peers: usize,
    max_established: usize,
    established: usize,
) -> Vec<&'a P> {
    let free = max_established.saturating_sub(established);
    if waiting_peers <= 2 * free {
        return Vec::new();
    }
    let mut drop = Vec::new();
    for c in peers {
        if drop.len() == 2 {
            break;
        }
        if c.closed() || !c.peer_choking() || !c.interested() {
            continue;
        }
        let mut since = c.choked_since();
        if let Some(li) = c.last_became_interested() {
            since = Some(match since {
                Some(s) if li > s => li,
                Some(s) => s,
                None => li,
            });
        }
        if let Some(s) = since {
            if age(clock, s) >= after {
                drop.push(c);
            }
        }
    }
    drop
}

/// Count connected / unchoked (Go `WebtorPeerCounts`).
pub fn peer_counts<P: PeerView>(peers: &[P]) -> (usize, usize) {
    let mut conns = 0;
    let mut unchoked = 0;
    for c in peers {
        if c.closed() {
            continue;
        }
        conns += 1;
        if !c.peer_choking() {
            unchoked += 1;
        }
    }
    (conns, unchoked)
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::webtor::clock::ManualClock;

    #[test]
    fn steal_skips_blocks_the_holder_has_likely_sent() {
        let chunk = 16 * 1024;
        // 1 MiB/s over 100 ms: 6.4 blocks in flight plus the one on the wire.
        let rate = 1024.0 * 1024.0;
        let rtt = Duration::from_millis(100);
        assert!(steal_lands_in_flight(0, rate, rtt, chunk));
        assert!(steal_lands_in_flight(7, rate, rtt, chunk));
        assert!(!steal_lands_in_flight(8, rate, rtt, chunk));
        // Unknown round trip: only the block at the front is protected.
        assert!(steal_lands_in_flight(0, rate, Duration::ZERO, chunk));
        assert!(!steal_lands_in_flight(1, rate, Duration::ZERO, chunk));
    }

    #[test]
    fn encode_focus_matches_go() {
        assert_eq!(encode_request_focus(-1), 0);
        assert_eq!(encode_request_focus(0), 1);
        assert_eq!(encode_request_focus(100), 101);
    }

    #[test]
    fn request_focus_index_basic() {
        let piece_len = 256 * 1024i64;
        let chunk = 16 * 1024u32;
        // offset into piece 1 at block 3
        let off = piece_len + 3 * i64::from(chunk);
        let stored = encode_request_focus(off);
        let idx = request_focus_index(stored, true, piece_len * 4, piece_len, chunk).unwrap();
        let chunks_per = (piece_len as u32) / chunk;
        assert_eq!(idx, chunks_per + 3);
    }

    #[test]
    fn peak_modes() {
        assert_eq!(next_peak_requests(PeakMode::Added, 16, 20, 1, false), 1);
        assert_eq!(
            next_peak_requests(PeakMode::WholeQueue, 16, 20, 1, false),
            20
        );
        assert_eq!(
            next_peak_requests(PeakMode::KeepOnTopUp, 16, 20, 1, false),
            16
        );
        assert_eq!(
            next_peak_requests(PeakMode::KeepOnTopUp, 16, 20, 4, true),
            20
        );
    }

    /// Go WebtorDropChokers: with free established slots, dial-pool pressure
    /// must exceed `2 * free` before any long-choking peer is eligible.
    #[test]
    fn drop_chokers_gate_spares_peers_when_free_slots() {
        let mut clock = ManualClock::new();
        let t0 = clock.now();
        clock.advance(Duration::from_secs(10));
        let choker = MockChoker {
            choking: true,
            interested: true,
            choked_since: Some(t0),
            last_interested: Some(t0),
            closed: false,
        };
        let peers = [choker];
        // free = 55 - 17 = 38; 2*free = 76; waiting 40 <= 76 → no drop
        assert!(
            chokers_to_drop(&clock, &peers, Duration::from_secs(5), 40, 55, 17).is_empty(),
            "must not kill live downloaders while established slots remain free"
        );
        // waiting 77 > 76 → gate opens
        assert_eq!(
            chokers_to_drop(&clock, &peers, Duration::from_secs(5), 77, 55, 17).len(),
            1
        );
        // At capacity (free=0) any waiting peer opens the gate
        assert_eq!(
            chokers_to_drop(&clock, &peers, Duration::from_secs(5), 1, 17, 17).len(),
            1
        );
        assert!(
            chokers_to_drop(&clock, &peers, Duration::from_secs(5), 0, 17, 17).is_empty(),
            "no waiting peers → never drop"
        );
    }

    struct MockChoker {
        choking: bool,
        interested: bool,
        choked_since: Option<std::time::Instant>,
        last_interested: Option<std::time::Instant>,
        closed: bool,
    }

    impl PeerView for MockChoker {
        type Id = u32;
        fn id(&self) -> u32 {
            1
        }
        fn download_rate_bps(&self) -> f64 {
            0.0
        }
        fn rtt(&self) -> Duration {
            Duration::ZERO
        }
        fn queued_requests(&self) -> u32 {
            0
        }
        fn cancelled_requests(&self) -> u32 {
            0
        }
        fn nominal_max_requests(&self) -> u32 {
            1
        }
        fn last_useful_data(&self) -> Option<std::time::Instant> {
            None
        }
        fn has_piece(&self, _: PieceIndex) -> bool {
            false
        }
        fn peer_choking(&self) -> bool {
            self.choking
        }
        fn closed(&self) -> bool {
            self.closed
        }
        fn interested(&self) -> bool {
            self.interested
        }
        fn choked_since(&self) -> Option<std::time::Instant> {
            self.choked_since
        }
        fn last_became_interested(&self) -> Option<std::time::Instant> {
            self.last_interested
        }
        fn valid_receive_count(&self, _: RequestIndex) -> u32 {
            0
        }
    }
}

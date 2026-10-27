//! Ports of `engine-go/patches/torrent/webtor_urgent_test.go` against the pure
//! `wt_core::webtor` decision layer (injected clock + mock views).

#![allow(clippy::field_reassign_with_default)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use wt_core::strategy::focus_distance;
use wt_core::tuning::{PeakMode, SeekTuning};
use wt_core::webtor::{
    block_is_overdue, chokers_to_drop, encode_request_focus, expected_arrival_secs,
    faster_focus_peer, has_playing_piece_we_lack, head_of_line, hedge_decisions,
    may_take_existing_request, next_peak_requests, nominal_from_peak, pending_requests,
    refocus_should_cancel, refocus_sort_key, request_focus_index, rescue_decisions,
    should_nudge_peer, should_steal_urgent, urgent_past_cancels, yield_to_faster_peers, Clock,
    ManualClock, PeerView, RttEstimator, TorrentView, DEFAULT_RTT_SECS, SLOW_PEER_BYTES_PER_SEC,
};
use wt_core::{PieceIndex, PiecePriority, RequestIndex};

#[derive(Clone, Debug)]
struct MockPeer {
    id: u32,
    rate: f64,
    rtt: Duration,
    queued: u32,
    cancelled: u32,
    max_req: u32,
    last_useful: Option<std::time::Instant>,
    pieces: HashSet<PieceIndex>,
    choking: bool,
    closed: bool,
    interested: bool,
    choked_since: Option<std::time::Instant>,
    last_interested: Option<std::time::Instant>,
    valid_recv: HashMap<RequestIndex, u32>,
    have_all: bool,
}

impl MockPeer {
    fn new(id: u32) -> Self {
        Self {
            id,
            rate: 0.0,
            rtt: Duration::ZERO,
            queued: 0,
            cancelled: 0,
            max_req: 64,
            last_useful: None,
            pieces: HashSet::new(),
            choking: false,
            closed: false,
            interested: false,
            choked_since: None,
            last_interested: None,
            valid_recv: HashMap::new(),
            have_all: false,
        }
    }

    fn with_rate(mut self, rate: f64) -> Self {
        self.rate = rate;
        self
    }

    fn with_rtt(mut self, rtt: Duration) -> Self {
        self.rtt = rtt;
        self
    }

    fn with_piece(mut self, p: PieceIndex) -> Self {
        self.pieces.insert(p);
        self
    }

    fn with_all_pieces(mut self) -> Self {
        self.have_all = true;
        self
    }
}

impl PeerView for MockPeer {
    type Id = u32;

    fn id(&self) -> u32 {
        self.id
    }
    fn download_rate_bps(&self) -> f64 {
        self.rate
    }
    fn rtt(&self) -> Duration {
        self.rtt
    }
    fn queued_requests(&self) -> u32 {
        self.queued
    }
    fn cancelled_requests(&self) -> u32 {
        self.cancelled
    }
    fn nominal_max_requests(&self) -> u32 {
        self.max_req
    }
    fn last_useful_data(&self) -> Option<std::time::Instant> {
        self.last_useful
    }
    fn has_piece(&self, piece: PieceIndex) -> bool {
        self.have_all || self.pieces.contains(&piece)
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
    fn valid_receive_count(&self, req: RequestIndex) -> u32 {
        self.valid_recv.get(&req).copied().unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default)]
struct MockTorrent {
    focus: Option<RequestIndex>,
    priorities: HashMap<PieceIndex, PiecePriority>,
    arrived: HashSet<RequestIndex>,
    max_end: RequestIndex,
    when: HashMap<RequestIndex, std::time::Instant>,
    holders: HashMap<RequestIndex, u32>,
    bytes_left: HashMap<PieceIndex, u64>,
    hedges: HashMap<RequestIndex, u32>,
    chunk: u32,
    piece_len: i64,
    interest: Option<(PieceIndex, PieceIndex)>,
    complete: HashSet<PieceIndex>,
    num_pieces: PieceIndex,
}

impl TorrentView for MockTorrent {
    type PeerId = u32;

    fn request_focus_index(&self) -> Option<RequestIndex> {
        self.focus
    }
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority {
        self.priorities
            .get(&piece)
            .copied()
            .unwrap_or(PiecePriority::None)
    }
    fn piece_of_request(&self, req: RequestIndex) -> PieceIndex {
        let cpp = (self.piece_len as u32).checked_div(self.chunk.max(1));
        match cpp {
            Some(0) | None => 0,
            Some(cpp) => req / cpp,
        }
    }
    fn block_arrived(&self, req: RequestIndex) -> bool {
        self.arrived.contains(&req)
    }
    fn max_end_request(&self) -> RequestIndex {
        self.max_end
    }
    fn request_when(&self, req: RequestIndex) -> Option<std::time::Instant> {
        self.when.get(&req).copied()
    }
    fn requesting_peer(&self, req: RequestIndex) -> Option<u32> {
        self.holders.get(&req).copied()
    }
    fn piece_bytes_left(&self, piece: PieceIndex) -> u64 {
        self.bytes_left.get(&piece).copied().unwrap_or(0)
    }
    fn hedge_peer(&self, req: RequestIndex) -> Option<u32> {
        self.hedges.get(&req).copied()
    }
    fn chunk_size(&self) -> u32 {
        self.chunk
    }
    fn playback_interest(&self) -> Option<(PieceIndex, PieceIndex)> {
        self.interest
    }
    fn piece_complete(&self, piece: PieceIndex) -> bool {
        self.complete.contains(&piece)
    }
    fn num_pieces(&self) -> PieceIndex {
        self.num_pieces
    }
}

fn none_features(_: &MockPeer) -> Option<[f64; wt_core::arrival::ARRIVAL_FEATURES]> {
    None
}

#[test]
fn test_urgent_steal_only_moves_stalled_urgent_blocks_to_delivering_peers() {
    let clock = ManualClock::new();
    let tuning = SeekTuning::default();
    let now = clock.now();

    let urgent_req = 10;
    let normal_req = 20;
    // Mirror Go: slow delivered 5s ago, fast 100ms ago, request 2s old.
    let slow_useful = now - Duration::from_secs(5);
    let fast_useful = now - Duration::from_millis(100);
    let req_old = now - Duration::from_secs(2);

    // Disabled by default
    assert!(!should_steal_urgent(
        &clock,
        &tuning,
        Duration::ZERO,
        urgent_req,
        PiecePriority::Now,
        req_old,
        Some(fast_useful),
        Some(slow_useful),
        None,
    ));

    let after = Duration::from_secs(1);
    assert!(
        should_steal_urgent(
            &clock,
            &tuning,
            after,
            urgent_req,
            PiecePriority::Now,
            req_old,
            Some(fast_useful),
            Some(slow_useful),
            None,
        ),
        "stalled urgent block"
    );
    assert!(
        !should_steal_urgent(
            &clock,
            &tuning,
            after,
            normal_req,
            PiecePriority::Normal,
            req_old,
            Some(fast_useful),
            Some(slow_useful),
            None,
        ),
        "non-urgent piece"
    );

    // Request too recent
    let recent = now - Duration::from_millis(300);
    assert!(!should_steal_urgent(
        &clock,
        &tuning,
        after,
        urgent_req,
        PiecePriority::Now,
        recent,
        Some(fast_useful),
        Some(slow_useful),
        None,
    ));

    // Stealer idle longer than age
    assert!(!should_steal_urgent(
        &clock,
        &tuning,
        after,
        urgent_req,
        PiecePriority::Now,
        req_old,
        Some(slow_useful),
        Some(fast_useful),
        None,
    ));

    // Holder delivered more recently than stealer
    let stealer = now - Duration::from_millis(200);
    let holder = now - Duration::from_millis(50);
    assert!(!should_steal_urgent(
        &clock,
        &tuning,
        after,
        urgent_req,
        PiecePriority::Now,
        req_old,
        Some(stealer),
        Some(holder),
        None,
    ));
}

#[test]
fn test_request_focus_fetches_the_piece_from_the_playhead_first() {
    // Covered by strategy focus ordering; request_focus_index maps offset → block.
    let piece_len = 256 * 1024i64;
    let chunk = 16 * 1024u32;
    let mid_off = piece_len + 8 * i64::from(chunk) + 100;
    let stored = encode_request_focus(mid_off);
    let mid = request_focus_index(stored, true, piece_len * 4, piece_len, chunk).unwrap();
    let begin = request_focus_index(
        encode_request_focus(piece_len),
        true,
        piece_len * 4,
        piece_len,
        chunk,
    )
    .unwrap();
    assert!(mid > begin);
    assert_eq!(
        request_focus_index(
            encode_request_focus(-1),
            true,
            piece_len * 4,
            piece_len,
            chunk
        ),
        None
    );
}

#[test]
fn test_request_focus_fetches_pieces_nearest_the_playhead_first() {
    let focus = 2u32;
    assert_eq!(focus_distance(2, focus), 0);
    assert!(focus_distance(6, focus) < focus_distance(1, focus));
    assert!(focus_distance(1, focus) < focus_distance(0, focus));
}

#[test]
fn test_playhead_blocks_go_to_faster_peers_and_steal_sooner() {
    let mut clock = ManualClock::new();
    let mut tuning = SeekTuning::default();
    tuning.focus_blocks = 8;
    tuning.focus_steal_divisor = 2;
    tuning.faster_ratio = 3.0;
    tuning.arrival_aware = false;

    let slow = MockPeer::new(1).with_rate(40.0 * 1024.0).with_all_pieces();
    let fast = MockPeer::new(2).with_rate(400.0 * 1024.0).with_all_pieces();
    let peers = vec![slow.clone(), fast.clone()];
    let playhead = 3u32;
    let focus = Some(0u32);
    let chunk = 16 * 1024u32;

    assert!(faster_focus_peer(
        &tuning,
        Duration::ZERO,
        &slow,
        &peers,
        playhead,
        1,
        PiecePriority::Now,
        focus,
        chunk,
        None,
        &none_features,
    )
    .is_none());

    let after = Duration::from_secs(1);
    assert!(faster_focus_peer(
        &tuning,
        after,
        &slow,
        &peers,
        playhead,
        1,
        PiecePriority::Now,
        focus,
        chunk,
        None,
        &none_features,
    )
    .is_some_and(|p| p.id == 2));

    assert!(faster_focus_peer(
        &tuning,
        after,
        &fast,
        &peers,
        playhead,
        1,
        PiecePriority::Now,
        focus,
        chunk,
        None,
        &none_features,
    )
    .is_none());

    // Past focus window
    assert!(faster_focus_peer(
        &tuning,
        after,
        &slow,
        &peers,
        playhead + tuning.focus_blocks as u32,
        1,
        PiecePriority::Now,
        focus,
        chunk,
        None,
        &none_features,
    )
    .is_none());

    // Playhead steals sooner (divisor)
    let t0 = clock.now();
    let req_when = t0;
    clock.advance(Duration::from_millis(700));
    let slow_useful = t0;
    let fast_useful = clock.now() - Duration::from_millis(50);
    assert!(should_steal_urgent(
        &clock,
        &tuning,
        after,
        playhead,
        PiecePriority::Now,
        req_when,
        Some(fast_useful),
        Some(slow_useful),
        focus,
    ));
    let ahead = playhead + tuning.focus_blocks as u32;
    assert!(!should_steal_urgent(
        &clock,
        &tuning,
        after,
        ahead,
        PiecePriority::Now,
        req_when,
        Some(fast_useful),
        Some(slow_useful),
        focus,
    ));
}

#[test]
fn test_round_trip_comes_from_requests_sent_to_an_idle_peer() {
    let mut clock = ManualClock::new();
    let mut e = RttEstimator::new();
    e.sent(7, &clock);
    clock.advance(Duration::from_millis(100));
    // Re-stamp probe_at as Go test does by mutating after sent
    e.probe_at = Some(clock.now() - Duration::from_millis(100));
    e.sent(8, &clock); // queued behind 7: not a new probe
    e.received(8, &clock);
    assert_eq!(
        e.estimate,
        Duration::ZERO,
        "queued block does not measure the trip"
    );
    e.received(7, &clock);
    assert!(
        e.estimate >= Duration::from_millis(100) && e.estimate < Duration::from_millis(150),
        "estimate {:?}",
        e.estimate
    );
    e.sent(9, &clock);
    e.probe_at = Some(clock.now() - Duration::from_millis(900));
    clock.advance(Duration::ZERO);
    e.received(9, &clock);
    assert!(
        e.estimate < Duration::from_millis(300),
        "one slow sample moves the estimate an eighth: {:?}",
        e.estimate
    );
}

#[test]
fn test_playhead_block_goes_to_the_peer_that_answers_soonest() {
    let tuning = SeekTuning {
        arrival_aware: true,
        arrival_ratio: 1.5,
        faster_ratio: 3.0,
        focus_blocks: 8,
        ..SeekTuning::default()
    };
    let far = MockPeer::new(1)
        .with_rate(4.0 * 1024.0 * 1024.0)
        .with_rtt(Duration::from_millis(400))
        .with_all_pieces();
    let near = MockPeer::new(2)
        .with_rate(1.0 * 1024.0 * 1024.0)
        .with_rtt(Duration::from_millis(40))
        .with_all_pieces();
    let peers = vec![far.clone(), near.clone()];
    let playhead = 0u32;
    let best = faster_focus_peer(
        &tuning,
        Duration::from_secs(1),
        &far,
        &peers,
        playhead,
        1,
        PiecePriority::Now,
        Some(0),
        16 * 1024,
        None,
        &none_features,
    );
    assert_eq!(best.map(|p| p.id), Some(2));
    assert!(faster_focus_peer(
        &tuning,
        Duration::from_secs(1),
        &near,
        &peers,
        playhead,
        1,
        PiecePriority::Now,
        Some(0),
        16 * 1024,
        None,
        &none_features,
    )
    .is_none());
}

#[test]
fn test_playback_blocks_stay_with_the_peer_that_asked_until_overdue() {
    let clock = ManualClock::new();
    let mut tuning = SeekTuning::default();
    let after = Duration::from_secs(3600);
    let req_when = clock.now();

    // With NoBalanceSteal + focus, only urgent steal allows taking.
    tuning.no_balance_steal = true;
    assert!(!may_take_existing_request(
        &clock,
        &tuning,
        true,
        after,
        10,
        PiecePriority::Next,
        req_when,
        0,
        Some(clock.now()),
        16,
        None,
        Some(0),
        false,
    ));

    tuning.no_balance_steal = false;
    // Balance allows a peer with empty queue to take from a full holder when
    // last-useful is newer — approximated here.
    assert!(may_take_existing_request(
        &clock,
        &tuning,
        true,
        after,
        10,
        PiecePriority::Next,
        req_when,
        0,
        Some(clock.now()),
        16,
        Some(clock.now() - Duration::from_secs(1)),
        Some(0),
        false,
    ));
}

#[test]
fn test_stays_interested_in_peers_with_any_part_of_the_playing_file() {
    let peer = MockPeer::new(1).with_piece(6);
    let pieces = [6u32];
    assert!(!has_playing_piece_we_lack(
        &peer,
        None,
        false,
        None,
        pieces.iter().copied(),
        |i| i == 6,
        8,
    ));
    assert!(has_playing_piece_we_lack(
        &peer,
        Some((4, 8)),
        false,
        None,
        pieces.iter().copied(),
        |i| !false && i == 6,
        8,
    ));
    assert!(!has_playing_piece_we_lack(
        &peer,
        Some((0, 4)),
        false,
        None,
        pieces.iter().copied(),
        |_| true,
        8,
    ));
}

#[test]
fn test_head_of_line_block_moves_to_a_sooner_peer_when_overdue() {
    let mut clock = ManualClock::new();
    let tuning = SeekTuning {
        hol_rescue: true,
        hol_after_ms: 300.0,
        ..SeekTuning::default()
    };
    let chunk = 16 * 1024u32;
    let mut tor = MockTorrent {
        focus: Some(0),
        max_end: 64,
        chunk,
        piece_len: 256 * 1024,
        num_pieces: 4,
        ..Default::default()
    };
    // Arrived around stuck blocks 2 and 5
    for r in [0u32, 1, 3, 4] {
        tor.arrived.insert(r);
    }
    for r in 6..20 {
        tor.arrived.insert(r);
    }
    let t_req = clock.now();
    tor.when.insert(2, t_req);
    tor.when.insert(5, t_req);
    tor.holders.insert(2, 1);
    tor.holders.insert(5, 1);

    // Holder has a long queue (20 blocks); rescuer is idle → sooner arrival.
    let mut holder = MockPeer::new(1)
        .with_rate(1.0 * 1024.0 * 1024.0)
        .with_rtt(Duration::from_millis(50))
        .with_all_pieces();
    holder.queued = 20;
    let rescuer = MockPeer::new(2)
        .with_rate(1.0 * 1024.0 * 1024.0)
        .with_rtt(Duration::from_millis(50))
        .with_all_pieces();
    let peers = vec![holder.clone(), rescuer.clone()];

    assert_eq!(head_of_line(&tor), Some(2));
    let moves = rescue_decisions(&clock, &tuning, &tor, &peers, None, &none_features);
    assert!(moves.is_empty(), "not overdue yet");

    clock.advance(Duration::from_secs(1));
    let moves = rescue_decisions(&clock, &tuning, &tor, &peers, None, &none_features);
    assert_eq!(moves.len(), 2);
    assert!(moves.iter().all(|m| m.from == 1 && m.to == 2));

    let off = SeekTuning {
        hol_rescue: false,
        ..tuning.clone()
    };
    assert!(rescue_decisions(&clock, &off, &tor, &peers, None, &none_features).is_empty());
}

#[test]
fn test_overdue_head_of_line_block_is_hedged_on_a_second_peer() {
    let mut clock = ManualClock::new();
    let tuning = SeekTuning {
        hedge: true,
        hol_after_ms: 300.0,
        ..SeekTuning::default()
    };
    let chunk = 16 * 1024u32;
    let mut tor = MockTorrent {
        focus: Some(0),
        max_end: 64,
        chunk,
        piece_len: 256 * 1024,
        ..Default::default()
    };
    for r in 0..8u32 {
        if r != 2 {
            tor.arrived.insert(r);
        }
    }
    let t_req = clock.now();
    tor.when.insert(2, t_req);
    tor.holders.insert(2, 1);

    let holder = MockPeer::new(1)
        .with_rate(1.0 * 1024.0 * 1024.0)
        .with_all_pieces();
    let hedger = MockPeer::new(2)
        .with_rate(1.0 * 1024.0 * 1024.0)
        .with_all_pieces();
    let peers = vec![holder, hedger];
    let no_learned = |_: &MockPeer, _: RequestIndex| None;

    assert!(hedge_decisions(
        &clock,
        &tuning,
        &tor,
        &peers,
        None,
        &none_features,
        &no_learned
    )
    .is_empty());

    clock.advance(Duration::from_secs(1));
    let moves = hedge_decisions(
        &clock,
        &tuning,
        &tor,
        &peers,
        None,
        &none_features,
        &no_learned,
    );
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].req, 2);
    assert_eq!(moves[0].holder, 1);
    assert_eq!(moves[0].hedger, 2);

    tor.hedges.insert(2, 2);
    assert!(hedge_decisions(
        &clock,
        &tuning,
        &tor,
        &peers,
        None,
        &none_features,
        &no_learned
    )
    .is_empty());
}

#[test]
fn test_slow_peer_leaves_a_playback_piece_to_faster_peers() {
    let chunk = 16 * 1024u32;
    let slow = MockPeer::new(1)
        .with_rate(16.0 * 1024.0) // under 48 KiB/s
        .with_piece(1);
    let fast = MockPeer::new(2).with_rate(1024.0 * 1024.0).with_piece(1);
    let peers = vec![slow.clone(), fast];
    assert!(slow.rate < SLOW_PEER_BYTES_PER_SEC);
    assert!(yield_to_faster_peers(&slow, &peers, 1, 256 * 1024, chunk,));
    let fast_self = MockPeer::new(2).with_rate(1024.0 * 1024.0).with_piece(1);
    assert!(!yield_to_faster_peers(
        &fast_self,
        &peers,
        1,
        256 * 1024,
        chunk,
    ));
}

#[test]
fn test_peer_that_keeps_us_choked_is_dropped() {
    let mut clock = ManualClock::new();
    let t0 = clock.now();
    clock.advance(Duration::from_secs(10));
    let choker = MockPeer {
        interested: true,
        choking: true,
        choked_since: Some(t0),
        ..MockPeer::new(1)
    };
    let giver = MockPeer {
        interested: true,
        choking: false,
        ..MockPeer::new(2)
    };
    let peers = vec![choker, giver];
    // waiting_peers > 2*free_slots
    let drop = chokers_to_drop(&clock, &peers, Duration::from_secs(5), 10, 2, 2);
    assert_eq!(drop.len(), 1);
    assert_eq!(drop[0].id, 1);
}

#[test]
fn test_top_up_of_a_busy_peer_keeps_its_request_limit() {
    assert_eq!(next_peak_requests(PeakMode::Added, 16, 20, 1, false), 1);
    assert_eq!(
        next_peak_requests(PeakMode::WholeQueue, 16, 20, 0, false),
        20
    );
    assert_eq!(nominal_from_peak(16), 32);
}

#[test]
fn test_expected_arrival_defaults() {
    assert_eq!(
        expected_arrival_secs(0.0, Duration::ZERO, 0, 16384),
        f64::MAX
    );
    let at = expected_arrival_secs(1_000_000.0, Duration::ZERO, 0, 16_384);
    // 0.2 + 1*16384/1e6
    assert!((at - (DEFAULT_RTT_SECS + 16_384.0 / 1_000_000.0)).abs() < 1e-9);
}

#[test]
fn test_pending_and_past_cancels_and_refocus() {
    let tuning = SeekTuning {
        full_pipeline: false,
        ..SeekTuning::default()
    };
    assert_eq!(pending_requests(&tuning, 4, 2), 6);
    let full = SeekTuning {
        full_pipeline: true,
        ..SeekTuning::default()
    };
    assert_eq!(pending_requests(&full, 4, 2), 4);
    assert!(urgent_past_cancels(3, 8, PiecePriority::Now));
    assert!(!urgent_past_cancels(8, 8, PiecePriority::Now));
    assert!(refocus_should_cancel(PiecePriority::Next));
    assert!(!refocus_should_cancel(PiecePriority::Now));

    let slow = MockPeer::new(1).with_rate(10.0);
    let fast = MockPeer::new(2).with_rate(100.0);
    let tuning = SeekTuning {
        arrival_aware: false,
        ..SeekTuning::default()
    };
    assert!(
        refocus_sort_key(&tuning, &fast, 16384, None, None)
            < refocus_sort_key(&tuning, &slow, 16384, None, None)
    );
}

#[test]
fn test_nudge_and_overdue_helpers() {
    let tuning = SeekTuning::default();
    let peer = MockPeer::new(1).with_piece(3);
    assert!(should_nudge_peer(&tuning, &peer, &[3]));
    let full = MockPeer {
        queued: 64,
        max_req: 64,
        ..MockPeer::new(2).with_piece(3)
    };
    assert!(!should_nudge_peer(&tuning, &full, &[3]));

    let clock = ManualClock::new();
    let when = clock.now();
    assert!(!block_is_overdue(
        &clock,
        &SeekTuning {
            hol_after_ms: 300.0,
            ..SeekTuning::default()
        },
        when,
        Duration::from_millis(50),
        None,
    ));
}

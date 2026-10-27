//! Desired peer request state — selection half of `requesting.go`.
//!
//! Builds the candidate request list and interest flag. Applying/stealing
//! requests against the live peer stays in agy-2's peer_conn layer.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Instant;

use wt_types::{PiecePriority, RequestIndex};

use super::order::{get_requestable_pieces, StrategyLimits, TorrentPieceQuery};
use super::piece_request_order::{PieceRequestOrder, PieceRequestOrderState};
use super::webtor::{candidate_budget, focus_distance, RequestTuning};

/// Opaque peer identity for assignment bookkeeping (agy-2 chooses the type).
pub type PeerId = u64;

/// Who holds an outstanding request, and when it was placed.
#[derive(Clone, Copy, Debug)]
pub struct PeerRequestAssignment {
    pub peer: PeerId,
    pub when: Instant,
    /// Outstanding request count on that peer (for steal ranking).
    pub peer_outstanding: u64,
}

/// Torrent-side snapshot for desired-request computation.
#[derive(Clone, Debug)]
pub struct RequestTorrentSnapshot {
    pub info_hash: [u8; 20],
    pub piece_length: i64,
    pub num_pieces: u32,
    /// `should_request` / `count_unverified` per piece.
    pub piece_requestable: Vec<bool>,
    pub piece_count_unverified: Vec<bool>,
    /// Undirtied request indexes per piece (precomputed by caller).
    pub undirtied_by_piece: Vec<Vec<RequestIndex>>,
    /// Map request index → piece index.
    pub piece_of_request: HashMap<RequestIndex, u32>,
    /// Current holders of outstanding requests.
    pub assignments: HashMap<RequestIndex, PeerRequestAssignment>,
    /// Request-index focus (playhead), if any.
    pub focus_request: Option<RequestIndex>,
    pub has_active_webseed_requests: bool,
    pub limits: StrategyLimits,
}

impl TorrentPieceQuery for RequestTorrentSnapshot {
    fn piece_length(&self, _: &[u8; 20]) -> i64 {
        self.piece_length
    }
    fn should_request(&self, _: &[u8; 20], piece: u32) -> bool {
        self.piece_requestable
            .get(piece as usize)
            .copied()
            .unwrap_or(false)
    }
    fn count_unverified(&self, _: &[u8; 20], piece: u32) -> bool {
        self.piece_count_unverified
            .get(piece as usize)
            .copied()
            .unwrap_or(false)
    }
}

/// Peer-side snapshot.
#[derive(Clone, Debug)]
pub struct DesiredPeerSnapshot {
    pub peer_id: PeerId,
    /// Bit-set or sparse: piece → has.
    pub has_piece: Vec<bool>,
    pub peer_choking: bool,
    pub peer_allowed_fast: Vec<bool>,
    pub outstanding: Vec<RequestIndex>,
    pub cancelled: Vec<RequestIndex>,
    pub nominal_max_requests: u32,
}

impl DesiredPeerSnapshot {
    fn has(&self, piece: u32) -> bool {
        self.has_piece.get(piece as usize).copied().unwrap_or(false)
    }
    fn allowed_fast(&self, piece: u32) -> bool {
        self.peer_allowed_fast
            .get(piece as usize)
            .copied()
            .unwrap_or(false)
    }
    fn has_outstanding(&self, r: RequestIndex) -> bool {
        self.outstanding.contains(&r)
    }
    fn has_cancelled(&self, r: RequestIndex) -> bool {
        self.cancelled.contains(&r)
    }
}

/// Result of desired-request computation for one peer.
#[derive(Clone, Debug, Default)]
pub struct DesiredRequestState {
    pub request_indexes: Vec<RequestIndex>,
    pub interested: bool,
    pub piece_states: HashMap<u32, PieceRequestOrderState>,
}

/// Collect candidate requests + interest for `peer` over `order`.
///
/// Does not sort; call [`sort_requests_for_peer`] before applying.
pub fn compute_desired_requests(
    torrent: &RequestTorrentSnapshot,
    peer: &DesiredPeerSnapshot,
    order: &PieceRequestOrder,
    tuning: &RequestTuning,
) -> DesiredRequestState {
    let mut desired = DesiredRequestState::default();
    let enough = candidate_budget(peer.nominal_max_requests, tuning.bound_candidates);
    let mut usable = 0usize;

    get_requestable_pieces(
        order,
        &torrent.limits,
        torrent,
        |ih, piece_index, piece_extra| {
            if ih != torrent.info_hash {
                return true;
            }
            // Stop scanning lower-priority pieces once we have enough free blocks.
            if enough > 0 && usable >= enough && piece_extra.priority < PiecePriority::Now {
                return false;
            }
            if !peer.has(piece_index) {
                return true;
            }
            desired.piece_states.insert(piece_index, piece_extra);
            let allowed_fast = peer.allowed_fast(piece_index);
            if let Some(reqs) = torrent.undirtied_by_piece.get(piece_index as usize) {
                for &r in reqs {
                    if !allowed_fast {
                        desired.interested = true;
                        if peer.peer_choking && !peer.has_outstanding(r) {
                            continue;
                        }
                    }
                    if peer.has_cancelled(r) {
                        continue;
                    }
                    desired.request_indexes.push(r);
                    if enough > 0 {
                        let holder = torrent.assignments.get(&r).map(|a| a.peer);
                        if holder.is_none() || holder == Some(peer.peer_id) {
                            usable += 1;
                        }
                    }
                }
            }
            true
        },
    );

    desired
}

/// Sort candidates with Go `desiredPeerRequests.lessByValue` (best first).
pub fn sort_requests_for_peer(
    requests: &mut [RequestIndex],
    torrent: &RequestTorrentSnapshot,
    peer: &DesiredPeerSnapshot,
    piece_states: &HashMap<u32, PieceRequestOrderState>,
) {
    requests.sort_by(
        |a, b| match request_less_by_value(*a, *b, torrent, peer, piece_states) {
            true => Ordering::Less,
            false => Ordering::Greater,
        },
    );
}

/// Returns true when `left` is a better request than `right` (should come first).
pub fn request_less_by_value(
    left: RequestIndex,
    right: RequestIndex,
    torrent: &RequestTorrentSnapshot,
    peer: &DesiredPeerSnapshot,
    piece_states: &HashMap<u32, PieceRequestOrderState>,
) -> bool {
    let left_piece = match torrent.piece_of_request.get(&left) {
        Some(&p) => p,
        None => return false,
    };
    let right_piece = match torrent.piece_of_request.get(&right) {
        Some(&p) => p,
        None => return true,
    };

    // Accumulator mirroring multiless: first differentiating comparison wins.
    let mut decided: Option<bool> = None;
    macro_rules! consider {
        ($less:expr, $same:expr) => {
            if decided.is_none() && !$same {
                decided = Some($less);
            }
        };
    }

    if peer.peer_choking {
        let l_af = !peer.allowed_fast(left_piece);
        let r_af = !peer.allowed_fast(right_piece);
        // Bool: false before true → allowed-fast pieces first.
        consider!(r_af, l_af == r_af);
    }

    let left_pri = piece_states
        .get(&left_piece)
        .map(|s| s.priority)
        .unwrap_or(PiecePriority::None);
    let right_pri = piece_states
        .get(&right_piece)
        .map(|s| s.priority)
        .unwrap_or(PiecePriority::None);
    // Higher priority first: Int(-leftPri, -rightPri).
    consider!((right_pri as u8) < (left_pri as u8), left_pri == right_pri);

    let left_asg = torrent.assignments.get(&left);
    let right_asg = torrent.assignments.get(&right);
    let left_peer = left_asg.map(|a| a.peer);
    let right_peer = right_asg.map(|a| a.peer);

    // Prefer chunks already requested from this peer.
    // Go: Bool(rightPeer == p, leftPeer == p) → false-before-true ⇒ left==p wins.
    consider!(
        left_peer == Some(peer.peer_id),
        (left_peer == Some(peer.peer_id)) == (right_peer == Some(peer.peer_id))
    );
    // Prefer unrequested chunks.
    // Go: Bool(rightPeer == nil, leftPeer == nil).
    consider!(
        left_peer.is_none(),
        left_peer.is_none() == right_peer.is_none()
    );

    if decided.is_none() && left_peer.is_some() && right_peer.is_some() {
        let l_out = left_asg.map(|a| a.peer_outstanding).unwrap_or(0);
        let r_out = right_asg.map(|a| a.peer_outstanding).unwrap_or(0);
        consider!(r_out < l_out, r_out == l_out);
        let l_when = left_asg.map(|a| a.when);
        let r_when = right_asg.map(|a| a.when);
        if let (Some(lw), Some(rw)) = (l_when, r_when) {
            // Most recently requested on the left (better cancel candidate).
            consider!(rw < lw, rw == lw);
        }
    }

    if let Some(focus_r) = torrent.focus_request {
        let focus_piece = torrent.piece_of_request.get(&focus_r).copied().unwrap_or(0);
        if left_piece == right_piece {
            if left_piece == focus_piece {
                // Within focus piece: prefer requests at/after focus.
                // Go: Bool(left < focus, right < focus) → false first.
                consider!(right < focus_r, (left < focus_r) == (right < focus_r));
            }
        } else {
            let ld = focus_distance(left_piece, focus_piece);
            let rd = focus_distance(right_piece, focus_piece);
            consider!(ld < rd, ld == rd);
        }
    }

    if torrent.has_active_webseed_requests {
        // Prefer highest request index (meet webseeds in the middle).
        consider!(left > right, left == right);
    } else {
        // Webtor streams: file order.
        consider!(left_piece < right_piece, left_piece == right_piece);
        consider!(left < right, left == right);
    }

    decided.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::super::piece_request_order::{
        PieceRequestOrder, PieceRequestOrderKey, PieceRequestOrderState,
    };
    use super::*;
    use wt_types::PiecePriority;

    fn setup_torrent(n_pieces: u32, chunk_per_piece: u32) -> RequestTorrentSnapshot {
        let mut undirtied = Vec::new();
        let mut piece_of = HashMap::new();
        let mut req = 0u32;
        for p in 0..n_pieces {
            let mut rs = Vec::new();
            for _ in 0..chunk_per_piece {
                rs.push(req);
                piece_of.insert(req, p);
                req += 1;
            }
            undirtied.push(rs);
        }
        RequestTorrentSnapshot {
            info_hash: [9u8; 20],
            piece_length: 64 * 1024,
            num_pieces: n_pieces,
            piece_requestable: vec![true; n_pieces as usize],
            piece_count_unverified: vec![false; n_pieces as usize],
            undirtied_by_piece: undirtied,
            piece_of_request: piece_of,
            assignments: HashMap::new(),
            focus_request: None,
            has_active_webseed_requests: false,
            limits: StrategyLimits::default(),
        }
    }

    #[test]
    fn desired_marks_interest_and_collects() {
        let torrent = setup_torrent(4, 2);
        let mut order = PieceRequestOrder::with_capacity(4);
        for i in 0..4u32 {
            order.add(
                PieceRequestOrderKey {
                    info_hash: torrent.info_hash,
                    index: i,
                },
                PieceRequestOrderState {
                    availability: 1,
                    priority: PiecePriority::Normal,
                    partial: false,
                },
            );
        }
        let peer = DesiredPeerSnapshot {
            peer_id: 1,
            has_piece: vec![true; 4],
            peer_choking: false,
            peer_allowed_fast: vec![false; 4],
            outstanding: vec![],
            cancelled: vec![],
            nominal_max_requests: 64,
        };
        let tuning = RequestTuning {
            bound_candidates: false,
            ..RequestTuning::default()
        };
        let desired = compute_desired_requests(&torrent, &peer, &order, &tuning);
        assert!(desired.interested);
        assert_eq!(desired.request_indexes.len(), 8);
    }

    #[test]
    fn less_by_value_prefers_higher_priority() {
        let mut torrent = setup_torrent(2, 1);
        let peer = DesiredPeerSnapshot {
            peer_id: 1,
            has_piece: vec![true; 2],
            peer_choking: false,
            peer_allowed_fast: vec![false; 2],
            outstanding: vec![],
            cancelled: vec![],
            nominal_max_requests: 8,
        };
        let mut states = HashMap::new();
        states.insert(
            0,
            PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Normal,
                partial: false,
            },
        );
        states.insert(
            1,
            PieceRequestOrderState {
                availability: 1,
                priority: PiecePriority::Now,
                partial: false,
            },
        );
        // request 1 is piece 1 (Now) — should be less than request 0 (Normal)
        assert!(request_less_by_value(1, 0, &torrent, &peer, &states));
        assert!(!request_less_by_value(0, 1, &torrent, &peer, &states));
        let _ = &mut torrent;
    }

    #[test]
    fn focus_distance_orders_near_playhead() {
        let mut torrent = setup_torrent(5, 1);
        torrent.focus_request = Some(2); // piece 2
        let peer = DesiredPeerSnapshot {
            peer_id: 1,
            has_piece: vec![true; 5],
            peer_choking: false,
            peer_allowed_fast: vec![false; 5],
            outstanding: vec![],
            cancelled: vec![],
            nominal_max_requests: 8,
        };
        let mut states = HashMap::new();
        for i in 0..5u32 {
            states.insert(
                i,
                PieceRequestOrderState {
                    availability: 1,
                    priority: PiecePriority::High,
                    partial: false,
                },
            );
        }
        // piece 3 (dist 1) before piece 4 (dist 2); piece 3 before piece 1 (behind)
        assert!(request_less_by_value(3, 4, &torrent, &peer, &states));
        assert!(request_less_by_value(3, 1, &torrent, &peer, &states));
    }
}

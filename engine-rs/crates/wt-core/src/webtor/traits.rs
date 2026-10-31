//! Views of peer / torrent state that decision functions need.
//!
//! agy-2 implements these for live `PeerConn` / `TorrentInner` (or passes
//! snapshots). Tests use the mock types in the webtor integration tests.

use std::time::{Duration, Instant};

use wt_types::{PieceIndex, PiecePriority, RequestIndex};

/// Peer fields the seek rules read. `Id` identifies a peer among candidates.
pub trait PeerView {
    type Id: Copy + Eq + std::fmt::Debug;

    fn id(&self) -> Self::Id;
    fn download_rate_bps(&self) -> f64;
    /// Measured RTT; `Duration::ZERO` means "not measured yet" (Go default).
    fn rtt(&self) -> Duration;
    /// Outstanding (uncancelled) request count.
    fn queued_requests(&self) -> u32;
    fn cancelled_requests(&self) -> u32;
    fn nominal_max_requests(&self) -> u32;
    fn last_useful_data(&self) -> Option<Instant>;
    fn has_piece(&self, piece: PieceIndex) -> bool;
    fn peer_choking(&self) -> bool;
    fn closed(&self) -> bool;
    fn allowed_fast(&self, piece: PieceIndex) -> bool {
        let _ = piece;
        false
    }
    /// AmInterested.
    fn interested(&self) -> bool {
        false
    }
    fn choked_since(&self) -> Option<Instant> {
        None
    }
    fn last_became_interested(&self) -> Option<Instant> {
        None
    }
    /// How many times this peer was allowed to receive `req` (hedge bookkeeping).
    fn valid_receive_count(&self, req: RequestIndex) -> u32 {
        let _ = req;
        0
    }
}

/// Torrent fields the seek rules read.
pub trait TorrentView {
    type PeerId: Copy + Eq + std::fmt::Debug;

    fn request_focus_index(&self) -> Option<RequestIndex>;
    /// A seek just moved the playhead and its eager hedge budget is not
    /// spent: hedge the first blocks at once.
    fn eager_hedge(&self) -> bool {
        false
    }
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority;
    fn piece_of_request(&self, req: RequestIndex) -> PieceIndex;
    fn block_arrived(&self, req: RequestIndex) -> bool;
    fn max_end_request(&self) -> RequestIndex;
    fn request_when(&self, req: RequestIndex) -> Option<Instant>;
    fn requesting_peer(&self, req: RequestIndex) -> Option<Self::PeerId>;
    fn piece_bytes_left(&self, piece: PieceIndex) -> u64;
    fn hashing_or_marking(&self, piece: PieceIndex) -> bool {
        let _ = piece;
        false
    }
    fn hedge_peer(&self, req: RequestIndex) -> Option<Self::PeerId> {
        let _ = req;
        None
    }
    fn chunk_size(&self) -> u32;
    /// Playback interest piece range `[begin, end)`, or `None` if unset.
    fn playback_interest(&self) -> Option<(PieceIndex, PieceIndex)> {
        None
    }
    fn piece_complete(&self, piece: PieceIndex) -> bool {
        let _ = piece;
        false
    }
    fn num_pieces(&self) -> PieceIndex {
        0
    }
}

/// Learned arrival predictor (agy-2's `arrival::ArrivalNet` / model).
pub trait ArrivalPredictor {
    /// Predict seconds until a block arrives given feature vector `x`.
    fn predict(&self, x: [f64; crate::arrival::ARRIVAL_FEATURES]) -> f64;
}

impl ArrivalPredictor for crate::arrival::ArrivalNet {
    fn predict(&self, x: [f64; crate::arrival::ARRIVAL_FEATURES]) -> f64 {
        crate::arrival::ArrivalNet::predict(self, x)
    }
}

impl ArrivalPredictor for crate::arrival::ArrivalModel {
    fn predict(&self, x: [f64; crate::arrival::ARRIVAL_FEATURES]) -> f64 {
        self.net.predict(x)
    }
}

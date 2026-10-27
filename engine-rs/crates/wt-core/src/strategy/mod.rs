//! # Request strategy (agy-2 integration guide)
//!
//! Pure, allocation-light piece request ordering and desired-request selection
//! ported from `engine-go/patches/torrent/internal/request-strategy` and the
//! selection half of `requesting.go` / `webtor_urgent.go`.
//!
//! ## What agy-2 should call
//!
//! Maintain one [`PieceRequestOrder`] per storage-capacity group (or per
//! torrent when uncapped). On priority / availability / partial changes:
//!
//! ```ignore
//! use wt_core::strategy::{
//!     PieceRequestOrder, PieceRequestOrderKey, PieceRequestOrderState,
//!     get_requestable_pieces, compute_desired_requests, DesiredPeerSnapshot,
//!     RequestTorrentSnapshot, StrategyLimits, candidate_budget, focus_distance,
//!     playback_pipeline_limit, RequestTuning,
//! };
//!
//! let mut order = PieceRequestOrder::with_capacity(num_pieces);
//! order.add(
//!     PieceRequestOrderKey { info_hash, index: 0 },
//!     PieceRequestOrderState { priority: PiecePriority::Now, availability: 3, partial: false },
//! );
//!
//! // Scan requestable pieces in Webtor order (priority desc, then piece index).
//! get_requestable_pieces(&order, &limits, |ih, piece, state| {
//!     /* peer has piece? collect undirtied request indexes */
//!     true // false = stop
//! });
//!
//! // Per-peer desired requests + interest (does not apply/steal; that stays in peer_conn).
//! let desired = compute_desired_requests(&torrent_snap, &peer_snap, &order, &tuning);
//! // desired.request_indexes is unsorted candidates; sort with desired.sort_for_peer(...)
//! // or use the returned heap-order via `sort_requests_for_peer`.
//! ```
//!
//! Webtor helpers: [`candidate_budget`], [`focus_distance`],
//! [`playback_pipeline_limit`], [`RequestTuning`] (mirrors SeekTuning fields
//! that affect selection only).
//!
//! Ordering rule (`piece_order_less`): **user priority (higher first), then
//! piece index (file/playhead order), then infohash**. Availability is stored
//! but not used for ordering (Webtor streaming change vs upstream rarest-first).

mod desired;
mod order;
mod piece_request_order;
mod webtor;

pub use desired::{
    compute_desired_requests, request_less_by_value, sort_requests_for_peer, DesiredPeerSnapshot,
    DesiredRequestState, PeerRequestAssignment, RequestTorrentSnapshot,
};
pub use order::{get_requestable_pieces, piece_order_cmp, StrategyLimits, TorrentPieceQuery};
pub use piece_request_order::{
    PieceRequestOrder, PieceRequestOrderItem, PieceRequestOrderKey, PieceRequestOrderState,
};
pub use webtor::{
    candidate_budget, focus_distance, playback_pipeline_limit, RequestTuning, DEFAULT_MIN_PIPELINE,
};

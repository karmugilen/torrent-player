//! # BEP 19 web seed peer (agy-2 integration guide)
//!
//! Ports `engine-go/patches/torrent/webseed-peer.go` and the Webtor urgent wake
//! from `webtor_urgent.go` (`webtorWakeWebseedsForPiece`). HTTP range I/O uses
//! `wt_discovery::webseed::WebSeedClient`.
//!
//! ## What agy-2 should call
//!
//! ```ignore
//! use wt_core::webseed_peer::{
//!     WebseedPeer, WebseedPeerConfig, WebseedChunk, WakeReason,
//!     DEFAULT_MAX_CONCURRENT, HOST_REQUEST_CONCURRENCY,
//! };
//! use wt_core::{CancelToken, PiecePriority, BLOCK_SIZE};
//! use wt_types::Info;
//!
//! let mut peer = WebseedPeer::new(WebseedPeerConfig {
//!     url: "https://example/seed/".into(),
//!     max_concurrent: DEFAULT_MAX_CONCURRENT,
//!     ..Default::default()
//! })?;
//! peer.set_info(info.clone());
//!
//! // Map request-index range → HTTP extent, stream 16 KiB chunks:
//! let chunks = peer.fetch_request_range(
//!     begin_ri, end_ri,
//!     |ri| torrent_offset_for_request(ri), // → (byte_offset, length)
//!     |ri| want_receive_chunk(ri),
//!     cancel.clone(),
//! ).await?;
//! for WebseedChunk { request_index, data } in chunks {
//!     receive_chunk(request_index, data)?;
//! }
//!
//! // On hash failure attributed to this webseed:
//! peer.provided_bad_data();
//!
//! // Webtor: after a seek raises a piece to Now, wake replanning:
//! if let Some(reason) = peer.wake_for_urgent_piece(piece, pure_priority, piece_complete) {
//!     schedule_immediate_webseed_update(reason);
//! }
//! ```
//!
//! Concurrency: `WebSeedClient` semaphore (`max_concurrent`, default 4) plus
//! host-level guidance constant `HOST_REQUEST_CONCURRENCY` (Go env default 25).
//! Errors trigger random backoff up to `error_backoff_max` (default 10s) and a
//! 1-minute convict suspension via [`WebseedPeer::convict`].

mod peer;

pub use peer::{
    request_index_to_extent, wake_for_urgent_piece, WakeReason, WebseedChunk, WebseedPeer,
    WebseedPeerConfig, WebseedPeerError, DEFAULT_ERROR_BACKOFF_MAX, DEFAULT_MAX_CONCURRENT,
    HOST_REQUEST_CONCURRENCY, MAX_DISCARD_BYTES,
};

//! # Piece hashing / verification (agy-2 integration guide)
//!
//! Bounded worker pool + v1 SHA-1 / v2 merkle verification, ported from
//! `engine-go/patches/torrent` (`torrent.go` piece hashers, `piece.go`
//! `VerifyData` / `VerifyDataContext`, `smartban`).
//!
//! ## What agy-2 should call
//!
//! ```ignore
//! use wt_core::hashing::{
//!     PieceHasherPool, HashJob, HashResult, ExpectedHash, SmartBanCache,
//!     hash_piece_sync, PieceCompletionCache, DEFAULT_HASHERS_PER_TORRENT,
//! };
//! use wt_core::{CancelToken, PiecePriority};
//! use std::sync::mpsc;
//!
//! // Per torrent (or share one pool across torrents with sticky workers):
//! let (tx, rx) = mpsc::channel();
//! let pool = PieceHasherPool::new(cfg.piece_hashers_per_torrent, tx);
//! pool.set_cancel_token(cancel.clone());
//!
//! // When a piece is fully written / needs re-check:
//! pool.queue(HashJob {
//!     piece_index: i,
//!     priority: PiecePriority::Now, // playback pieces hash first
//!     storage: piece_storage,
//!     expected: ExpectedHash::V1(info.piece_hash_v1(i).unwrap()),
//!     piece_length: info.piece_length,
//!     data_length: info.piece(i).length,
//!     v1_zero_pad: 0,
//!     smartban: Some(SmartBanPieceRange {
//!         block_begin: request_index_begin,
//!         chunk_size: BLOCK_SIZE,
//!     }),
//!     seq: next_verify_count,
//! })?;
//!
//! // On network runtime / torrent loop: drain results (never hash there).
//! while let Ok(r) = rx.try_recv() {
//!     // r.correct → mark_complete / pieceHashed; ban r.differing_peers
//! }
//!
//! // Sync path (tests / VerifyData without the pool):
//! let r = hash_piece_sync(&storage, &ExpectedHash::V1(h), len, piece_len, 0, None, &mut buf);
//!
//! // Cancel in-flight + queued work:
//! cancel.cancel();
//! ```
//!
//! Workers run on dedicated OS threads (not the tokio network runtime). Each
//! worker reuses one piece-sized buffer. Queue order: `PiecePriority` desc,
//! then lower `piece_index`, then lower `seq`.

mod completion;
mod pool;
mod sha1_backend;
mod smartban;
mod verify;

pub use completion::PieceCompletionCache;
pub use pool::{
    HashJob, HashQueueError, HashResult, PieceHasherPool, SmartBanPieceRange,
    DEFAULT_HASHERS_PER_TORRENT,
};
pub use sha1_backend::{
    hash_v1_default, hash_v1_with_pad, selected_backend, throughput_mib_s, PieceSha1, RingSha1,
    Sha1BackendKind, SoftSha1,
};
pub use smartban::{BannableAddr, SmartBanCache};
pub use verify::{hash_piece_sync, sha1_throughput_mib_s, ExpectedHash, HashWorkError};

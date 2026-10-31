//! # Piece state bookkeeping (agy-2 integration guide)
//!
//! Per-piece dirty/pending/requested chunk sets, completion cache, hashing
//! flags, priority combination, peer availability, `PieceStateRuns` RLE, and
//! bytes-completed / bytes-missing — ported from
//! `engine-go/patches/torrent/{piece.go,piecestate.go,torrent.go,file.go,t.go}`.
//!
//! ## What agy-2 should call
//!
//! Own one [`PieceBook`] per torrent (under the torrent/client lock, like Go).
//! Use [`StorageGlue`] for storage I/O so write errors hit the callback.
//!
//! ```ignore
//! use wt_core::piece_state::{
//!     PieceBook, PiecePublicState, PieceStateRun, raise_priority,
//!     combine_pure_priority, ChangeWatch,
//! };
//! use wt_core::storage_adapter::StorageGlue;
//!
//! let mut book = PieceBook::new(num_pieces, piece_length, total_length, BLOCK_SIZE);
//!
//! // On PIECE message after a successful write:
//! book.mark_chunk_dirty(req_index);
//! book.clear_chunk_requested(req_index);
//!
//! // When all chunks of a piece are dirty → queue hash (agy-2 / hashing pool).
//! if book.piece_all_dirty(piece) {
//!     book.queue_for_hash(piece);
//! }
//!
//! // After hash result (hashing crate reports):
//! storage.mark_complete(piece_obj, hash)?; // or mark_not_complete + pend_all_chunks
//! book.update_completion(piece, storage.completion(piece_obj, hash));
//!
//! // Priority: file / reader / piece max → effective (None while hashing/complete).
//! book.set_file_priority_span(f_begin, f_end, PiecePriority::Normal);
//! book.set_reader_now(&[playhead_piece]);
//! let prio = book.effective_priority(piece, true, false);
//!
//! // /pieces API:
//! let runs = book.piece_state_runs();
//!
//! // Progress:
//! let done = book.bytes_completed();
//! let left = book.bytes_missing();
//! ```
//!
//! Dirty semantics match Go: a chunk in `dirtyChunks` has been received and
//! written; "pending" means not dirty. Requested is a separate compact set for
//! outstanding peer requests (Go keeps peer/when in `requestState`; agy-2 may
//! keep that map and use this bitset for fast membership).
//!
//! `piece_state_runs` groups consecutive pieces with equal [`PiecePublicState`]
//! (Go `PieceStateRuns`). Display format matches Go `PieceStateRun.String`.

mod bitset;
mod book;
mod bytes;
mod chunk_bits;
mod piece;
mod priority;
mod runs;
mod watch;

pub use bitset::Bitset;
pub use book::PieceBook;
pub use bytes::{file_bytes_completed, file_bytes_left};
pub use chunk_bits::ChunkBits;
pub use piece::PieceFlags;
pub use priority::{combine_pure_priority, raise_priority};
pub use runs::{format_piece_state_run, PiecePublicState, PieceStateRun};
pub use watch::ChangeWatch;

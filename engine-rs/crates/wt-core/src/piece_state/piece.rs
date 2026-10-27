//! Per-piece flags cached beside the torrent-wide bitmaps.

use wt_types::{Completion, PiecePriority};

/// Mutable per-piece fields (Go `Piece` subset used for state / priority).
#[derive(Clone, Debug)]
pub struct PieceFlags {
    pub piece_priority: PiecePriority,
    /// Max file priority spanning this piece (updated via file spans).
    pub file_priority: PiecePriority,
    pub relative_availability: i32,
    pub hashing: bool,
    pub marking: bool,
    pub storage_completion_ok: bool,
    pub storage_completion_has_been_ok: bool,
    pub missing_piece_layer_hash: bool,
    /// Length of this piece in bytes.
    pub length: i64,
    /// Number of chunks in this piece.
    pub num_chunks: u32,
    /// First request index of this piece (torrent-wide).
    pub request_index_begin: u32,
}

impl PieceFlags {
    pub fn new(length: i64, num_chunks: u32, request_index_begin: u32) -> Self {
        Self {
            piece_priority: PiecePriority::None,
            file_priority: PiecePriority::None,
            relative_availability: 0,
            hashing: false,
            marking: false,
            storage_completion_ok: false,
            storage_completion_has_been_ok: false,
            missing_piece_layer_hash: false,
            length,
            num_chunks,
            request_index_begin,
        }
    }

    pub fn completion(&self, complete: bool) -> Completion {
        Completion {
            ok: self.storage_completion_ok,
            complete: self.storage_completion_ok && complete,
            err: None,
        }
    }

    /// Updates ok/has-been-ok flags. Returns `(ok_changed, now_complete)`.
    /// Caller owns the completed-pieces bitset.
    pub fn update_from_completion(&mut self, c: &Completion) -> (bool, bool) {
        let prev_ok = self.storage_completion_ok;
        self.storage_completion_ok = c.ok;
        if c.ok {
            self.storage_completion_has_been_ok = true;
        }
        (prev_ok != c.ok, c.ok && c.complete)
    }
}

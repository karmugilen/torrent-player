//! Cached piece completion state (Go `Piece.storageCompletionOk` / hashing flags).

use wt_types::{Completion, PieceStorage, StorageResult};

/// Per-piece completion cache so the torrent loop does not hit storage every tick.
#[derive(Clone, Debug, Default)]
pub struct PieceCompletionCache {
    pub storage_completion_ok: bool,
    pub storage_completion_has_been_ok: bool,
    pub complete: bool,
    pub hashing: bool,
    pub queued_for_hash: bool,
    pub num_verifies: u64,
}

impl PieceCompletionCache {






}

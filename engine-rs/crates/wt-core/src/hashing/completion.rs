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
    pub fn from_storage(c: Completion) -> Self {
        let mut s = Self::default();
        s.update_from_completion(c);
        s
    }

    pub fn update_from_completion(&mut self, c: Completion) {
        self.storage_completion_ok = c.ok;
        if c.ok {
            self.storage_completion_has_been_ok = true;
            self.complete = c.complete;
        }
    }

    pub fn update_from_storage(&mut self, storage: &dyn PieceStorage) {
        self.update_from_completion(storage.completion());
    }

    /// After a successful hash pass: mark complete in storage and cache.
    pub fn apply_hash_ok(&mut self, storage: &dyn PieceStorage) -> StorageResult<()> {
        storage.mark_complete()?;
        self.storage_completion_ok = true;
        self.storage_completion_has_been_ok = true;
        self.complete = true;
        self.hashing = false;
        self.queued_for_hash = false;
        self.num_verifies = self.num_verifies.saturating_add(1);
        Ok(())
    }

    /// After a failed hash (or I/O error treated as incomplete).
    pub fn apply_hash_bad(&mut self, storage: &dyn PieceStorage) -> StorageResult<()> {
        storage.mark_not_complete()?;
        self.storage_completion_ok = true;
        self.complete = false;
        self.hashing = false;
        self.queued_for_hash = false;
        self.num_verifies = self.num_verifies.saturating_add(1);
        Ok(())
    }

    /// Next novel verify count (Go `nextNovelHashCount`).
    pub fn next_novel_hash_count(&self) -> u64 {
        let mut n = self.num_verifies;
        if self.hashing {
            n = n.saturating_add(1);
        }
        n.saturating_add(1)
    }

    /// Go `ignoreForRequests` cheap checks without bitmap lookups.
    pub fn ignore_for_requests(&self, have_hash: bool, data_download_disallowed: bool) -> bool {
        if !self.storage_completion_ok {
            return true;
        }
        if self.hashing || !have_hash || data_download_disallowed {
            return true;
        }
        if self.complete {
            return true;
        }
        if self.queued_for_hash {
            return true;
        }
        false
    }
}

//! Torrent-level piece bookkeeping (dirty chunks, completion, priority, availability).

use wt_types::{Completion, PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};

use super::bitset::Bitset;
use super::piece::PieceFlags;
use super::priority::combine_pure_priority;
use super::runs::{encode_runs, PiecePublicState, PieceStateRun};

/// Per-torrent piece state. Caller serializes access (Go client lock).
#[derive(Clone, Debug)]
pub struct PieceBook {
    pub(crate) pieces: Vec<PieceFlags>,
    /// Received-but-unverified chunks (Go `dirtyChunks`), torrent-wide RequestIndex.
    dirty_chunks: Bitset,
    /// Outstanding peer request membership (compact; peer/when stay in torrent).
    requested_chunks: Bitset,
    /// Verified-complete pieces (Go `_completedPieces`).
    pub(crate) completed_pieces: Bitset,
    /// Pieces queued for hash (Go `piecesQueuedForHash`).
    queued_for_hash: Bitset,
    /// Reader Now interest set.
    reader_now: Bitset,
    /// Reader Readahead interest set.
    reader_readahead: Bitset,
    conns_with_all_pieces: i32,
    pub(crate) piece_length_usual: i64,
    pub(crate) total_length: i64,
    chunk_size: u32,
    change_version: u64,
}

impl PieceBook {
    pub fn new(num_pieces: u32, piece_length: i64, total_length: i64, chunk_size: u32) -> Self {
        let chunk_size = if chunk_size == 0 {
            BLOCK_SIZE
        } else {
            chunk_size
        };
        let mut pieces = Vec::with_capacity(num_pieces as usize);
        let mut req_begin = 0u32;
        for i in 0..num_pieces {
            let plen = piece_length_at(i, piece_length, total_length);
            let nchunks = num_chunks_for(plen, chunk_size);
            pieces.push(PieceFlags::new(plen, nchunks, req_begin));
            req_begin = req_begin.saturating_add(nchunks);
        }
        Self {
            pieces,
            dirty_chunks: Bitset::new(),
            requested_chunks: Bitset::new(),
            completed_pieces: Bitset::new(),
            queued_for_hash: Bitset::new(),
            reader_now: Bitset::new(),
            reader_readahead: Bitset::new(),
            conns_with_all_pieces: 0,
            piece_length_usual: piece_length,
            total_length,
            chunk_size,
            change_version: 1,
        }
    }

    pub fn num_pieces(&self) -> u32 {
        self.pieces.len() as u32
    }

    pub fn change_version(&self) -> u64 {
        self.change_version
    }

    pub fn bump_change(&mut self) -> u64 {
        self.change_version = self.change_version.saturating_add(1);
        self.change_version
    }

    // ---- geometry ----

    pub fn piece_length(&self, piece: PieceIndex) -> i64 {
        self.pieces
            .get(piece as usize)
            .map(|p| p.length)
            .unwrap_or(0)
    }

    pub fn piece_num_chunks(&self, piece: PieceIndex) -> u32 {
        self.pieces
            .get(piece as usize)
            .map(|p| p.num_chunks)
            .unwrap_or(0)
    }

    pub fn piece_request_index_begin(&self, piece: PieceIndex) -> RequestIndex {
        self.pieces
            .get(piece as usize)
            .map(|p| p.request_index_begin)
            .unwrap_or(0)
    }

    pub fn piece_request_index_end(&self, piece: PieceIndex) -> RequestIndex {
        let begin = self.piece_request_index_begin(piece);
        begin.saturating_add(self.piece_num_chunks(piece))
    }

    pub fn request_index_to_piece(&self, req: RequestIndex) -> PieceIndex {
        // Binary search by request_index_begin.
        let mut lo = 0usize;
        let mut hi = self.pieces.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let begin = self.pieces[mid].request_index_begin;
            let end = begin + self.pieces[mid].num_chunks;
            if req < begin {
                hi = mid;
            } else if req >= end {
                lo = mid + 1;
            } else {
                return mid as PieceIndex;
            }
        }
        lo.saturating_sub(1)
            .min(self.pieces.len().saturating_sub(1)) as PieceIndex
    }

    pub fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    // ---- dirty / pending ----

    pub fn mark_chunk_dirty(&mut self, req: RequestIndex) {
        self.dirty_chunks.insert(req);
        self.bump_change();
    }

    pub fn mark_chunk_pending(&mut self, req: RequestIndex) {
        self.dirty_chunks.remove(req);
        self.bump_change();
    }

    pub fn pend_all_chunks(&mut self, piece: PieceIndex) {
        let begin = self.piece_request_index_begin(piece);
        let end = self.piece_request_index_end(piece);
        self.dirty_chunks.remove_range(begin, end);
        self.bump_change();
    }

    pub fn chunk_is_dirty(&self, req: RequestIndex) -> bool {
        self.dirty_chunks.contains(req)
    }

    pub fn chunk_is_pending(&self, piece: PieceIndex, chunk_in_piece: u32) -> bool {
        let begin = self.piece_request_index_begin(piece);
        !self
            .dirty_chunks
            .contains(begin.saturating_add(chunk_in_piece))
    }

    pub fn piece_num_dirty_chunks(&self, piece: PieceIndex) -> u32 {
        let begin = self.piece_request_index_begin(piece);
        let end = self.piece_request_index_end(piece);
        self.dirty_chunks.range_cardinality(begin, end) as u32
    }

    pub fn piece_all_dirty(&self, piece: PieceIndex) -> bool {
        let n = self.piece_num_chunks(piece);
        n > 0 && self.piece_num_dirty_chunks(piece) == n
    }

    pub fn piece_has_dirty_chunks(&self, piece: PieceIndex) -> bool {
        self.piece_num_dirty_chunks(piece) != 0
    }

    /// Go `piecePartiallyDownloaded`: has some dirty chunks but not all, and not complete.
    pub fn piece_partially_downloaded(&self, piece: PieceIndex) -> bool {
        if self.piece_complete(piece) {
            return false;
        }
        if self.piece_all_dirty(piece) {
            return false;
        }
        self.piece_has_dirty_chunks(piece)
    }

    /// Go `Piece.numDirtyBytes`.
    pub fn num_dirty_bytes(&self, piece: PieceIndex) -> i64 {
        let Some(p) = self.pieces.get(piece as usize) else {
            return 0;
        };
        let nchunks = p.num_chunks;
        if nchunks == 0 {
            return 0;
        }
        let begin = p.request_index_begin;
        let mut num_regular = self.piece_num_dirty_chunks(piece);
        let mut ret = 0i64;
        let last = nchunks - 1;
        if self.dirty_chunks.contains(begin + last) {
            num_regular = num_regular.saturating_sub(1);
            let last_len = chunk_length(last, p.length, self.chunk_size);
            ret += i64::from(last_len);
        }
        ret += i64::from(num_regular) * i64::from(self.chunk_size);
        ret
    }

    pub fn iter_clean_chunk_indexes(&self, piece: PieceIndex) -> Vec<u32> {
        let begin = self.piece_request_index_begin(piece);
        let end = self.piece_request_index_end(piece);
        self.dirty_chunks
            .unset_in_range(begin, end)
            .into_iter()
            .map(|req| req.saturating_sub(begin))
            .collect()
    }

    pub fn first_clean_chunk(&self, piece: PieceIndex) -> Option<u32> {
        self.iter_clean_chunk_indexes(piece).into_iter().next()
    }

    // ---- requested ----

    pub fn mark_chunk_requested(&mut self, req: RequestIndex) {
        self.requested_chunks.insert(req);
    }

    pub fn clear_chunk_requested(&mut self, req: RequestIndex) {
        self.requested_chunks.remove(req);
    }

    pub fn chunk_is_requested(&self, req: RequestIndex) -> bool {
        self.requested_chunks.contains(req)
    }

    // ---- completion / hashing / marking ----

    pub fn completed_pieces(&self) -> &Bitset {
        &self.completed_pieces
    }

    pub fn piece_complete(&self, piece: PieceIndex) -> bool {
        self.completed_pieces.contains(piece)
    }

    pub fn storage_completion_ok(&self, piece: PieceIndex) -> bool {
        self.pieces
            .get(piece as usize)
            .map(|p| p.storage_completion_ok)
            .unwrap_or(false)
    }

    /// Update cached completion from a storage `Completion`. Returns whether the
    /// complete bit (or ok flag) changed.
    pub fn update_completion(&mut self, piece: PieceIndex, c: Completion) -> bool {
        let Some(p) = self.pieces.get_mut(piece as usize) else {
            return false;
        };
        let prev_ok = p.storage_completion_ok;
        let prev_complete = self.completed_pieces.contains(piece);
        p.storage_completion_ok = c.ok;
        if c.ok {
            p.storage_completion_has_been_ok = true;
        }
        let now_complete = c.ok && c.complete;
        if now_complete {
            self.completed_pieces.insert(piece);
        } else {
            self.completed_pieces.remove(piece);
        }
        let changed = prev_ok != c.ok || prev_complete != now_complete;
        if changed {
            self.bump_change();
        }
        changed
    }

    pub fn set_complete_cached(&mut self, piece: PieceIndex, ok: bool, complete: bool) -> bool {
        self.update_completion(
            piece,
            Completion {
                ok,
                complete,
                err: None,
            },
        )
    }

    pub fn set_hashing(&mut self, piece: PieceIndex, v: bool) {
        if let Some(p) = self.pieces.get_mut(piece as usize) {
            if p.hashing != v {
                p.hashing = v;
                self.bump_change();
            }
        }
    }

    pub fn set_marking(&mut self, piece: PieceIndex, v: bool) {
        if let Some(p) = self.pieces.get_mut(piece as usize) {
            if p.marking != v {
                p.marking = v;
                self.bump_change();
            }
        }
    }

    pub fn queue_for_hash(&mut self, piece: PieceIndex) {
        if self.queued_for_hash.insert(piece) {
            self.bump_change();
        }
    }

    pub fn clear_queued_for_hash(&mut self, piece: PieceIndex) {
        if self.queued_for_hash.remove(piece) {
            self.bump_change();
        }
    }

    pub fn queued_for_hash(&self, piece: PieceIndex) -> bool {
        self.queued_for_hash.contains(piece)
    }

    pub fn set_missing_piece_layer_hash(&mut self, piece: PieceIndex, v: bool) {
        if let Some(p) = self.pieces.get_mut(piece as usize) {
            p.missing_piece_layer_hash = v;
        }
    }

    // ---- priority ----

    pub fn set_piece_priority(&mut self, piece: PieceIndex, prio: PiecePriority) {
        if let Some(p) = self.pieces.get_mut(piece as usize) {
            if p.piece_priority != prio {
                p.piece_priority = prio;
                self.bump_change();
            }
        }
    }

    /// Set file priority for pieces in `[begin, end)`.
    pub fn set_file_priority_span(
        &mut self,
        begin: PieceIndex,
        end: PieceIndex,
        prio: PiecePriority,
    ) {
        let mut changed = false;
        for i in begin..end.min(self.num_pieces()) {
            if let Some(p) = self.pieces.get_mut(i as usize) {
                // Raise across files that share a piece (Go iterates files and Raise).
                let next = if prio > p.file_priority {
                    prio
                } else {
                    p.file_priority
                };
                // When setting a span we assign max with existing; for a full
                // rebuild caller should zero first via `clear_file_priorities`.
                if p.file_priority != next {
                    p.file_priority = next;
                    changed = true;
                }
            }
        }
        if changed {
            self.bump_change();
        }
    }

    pub fn clear_file_priorities(&mut self) {
        for p in &mut self.pieces {
            p.file_priority = PiecePriority::None;
        }
    }

    pub fn set_file_priority_exact(
        &mut self,
        begin: PieceIndex,
        end: PieceIndex,
        prio: PiecePriority,
    ) {
        for i in begin..end.min(self.num_pieces()) {
            if let Some(p) = self.pieces.get_mut(i as usize) {
                p.file_priority = prio;
            }
        }
        self.bump_change();
    }

    pub fn set_reader_now(&mut self, pieces: &[PieceIndex]) {
        self.reader_now.clear();
        for &p in pieces {
            self.reader_now.insert(p);
        }
        self.bump_change();
    }

    pub fn set_reader_readahead(&mut self, pieces: &[PieceIndex]) {
        self.reader_readahead.clear();
        for &p in pieces {
            self.reader_readahead.insert(p);
        }
        self.bump_change();
    }

    pub fn pure_priority(&self, piece: PieceIndex) -> PiecePriority {
        let Some(p) = self.pieces.get(piece as usize) else {
            return PiecePriority::None;
        };
        combine_pure_priority(
            p.piece_priority,
            p.file_priority,
            self.reader_now.contains(piece),
            self.reader_readahead.contains(piece),
        )
    }

    pub fn ignore_for_requests(
        &self,
        piece: PieceIndex,
        have_hash: bool,
        data_download_disallowed: bool,
    ) -> bool {
        let Some(p) = self.pieces.get(piece as usize) else {
            return true;
        };
        if !p.storage_completion_ok {
            return true;
        }
        if p.hashing || p.marking || !have_hash || data_download_disallowed {
            return true;
        }
        if self.piece_complete(piece) {
            return true;
        }
        if self.queued_for_hash(piece) {
            return true;
        }
        false
    }

    pub fn effective_priority(
        &self,
        piece: PieceIndex,
        have_hash: bool,
        data_download_disallowed: bool,
    ) -> PiecePriority {
        if self.ignore_for_requests(piece, have_hash, data_download_disallowed) {
            return PiecePriority::None;
        }
        self.pure_priority(piece)
    }

    // ---- availability ----

    pub fn note_peer_has_all(&mut self) {
        self.conns_with_all_pieces = self.conns_with_all_pieces.saturating_add(1);
        self.bump_change();
    }

    pub fn note_peer_lost_all(&mut self) {
        self.conns_with_all_pieces = self.conns_with_all_pieces.saturating_sub(1);
        self.bump_change();
    }

    pub fn increment_relative_availability(&mut self, piece: PieceIndex) {
        if let Some(p) = self.pieces.get_mut(piece as usize) {
            p.relative_availability = p.relative_availability.saturating_add(1);
            self.bump_change();
        }
    }

    pub fn decrement_relative_availability(&mut self, piece: PieceIndex) {
        if let Some(p) = self.pieces.get_mut(piece as usize) {
            p.relative_availability = p.relative_availability.saturating_sub(1);
            self.bump_change();
        }
    }

    pub fn availability(&self, piece: PieceIndex) -> i32 {
        let rel = self
            .pieces
            .get(piece as usize)
            .map(|p| p.relative_availability)
            .unwrap_or(0);
        self.conns_with_all_pieces.saturating_add(rel)
    }

    pub fn conns_with_all_pieces(&self) -> i32 {
        self.conns_with_all_pieces
    }

    // ---- public state / runs ----

    pub fn piece_state(&self, piece: PieceIndex) -> PiecePublicState {
        let Some(p) = self.pieces.get(piece as usize) else {
            return PiecePublicState::default();
        };
        let complete = self.piece_complete(piece);
        let queued = self.queued_for_hash(piece);
        let hashing = p.hashing;
        let completion = Completion {
            ok: p.storage_completion_ok,
            complete: p.storage_completion_ok && complete,
            err: None,
        };
        let mut partial = false;
        if completion.ok && !completion.complete && self.piece_partially_downloaded(piece) {
            partial = true;
        }
        // effective priority needs have_hash; public state uses effective with
        // have_hash=true by default (caller can recompute). Match Go pieceState
        // which uses effectivePriority() with live haveHash().
        let priority = self.effective_priority(piece, !p.missing_piece_layer_hash, false);
        PiecePublicState {
            completion,
            priority,
            checking: queued || hashing,
            hashing,
            queued_for_hash: queued,
            marking: p.marking,
            partial,
            missing_piece_layer_hash: p.missing_piece_layer_hash,
        }
    }

    /// Go `pieceStateRuns` — RLE over [`piece_state`].
    pub fn piece_state_runs(&self) -> Vec<PieceStateRun> {
        encode_runs((0..self.num_pieces()).map(|i| self.piece_state(i)))
    }

    /// Like [`piece_state_runs`] but with an explicit `have_hash` / download gate
    /// applied uniformly (tests / callers without per-piece v2 hash flags).
    pub fn piece_state_runs_with(
        &self,
        have_hash: bool,
        data_download_disallowed: bool,
    ) -> Vec<PieceStateRun> {
        encode_runs((0..self.num_pieces()).map(|i| {
            let mut st = self.piece_state(i);
            st.priority = self.effective_priority(i, have_hash, data_download_disallowed);
            st
        }))
    }
}

fn piece_length_at(index: PieceIndex, piece_length: i64, total_length: i64) -> i64 {
    if piece_length <= 0 {
        return 0;
    }
    let offset = i64::from(index) * piece_length;
    (total_length - offset).clamp(0, piece_length)
}

fn num_chunks_for(piece_len: i64, chunk_size: u32) -> u32 {
    if piece_len <= 0 || chunk_size == 0 {
        return 0;
    }
    (piece_len as u64).div_ceil(u64::from(chunk_size)) as u32
}

fn chunk_length(chunk_in_piece: u32, piece_len: i64, chunk_size: u32) -> u32 {
    let start = i64::from(chunk_in_piece) * i64::from(chunk_size);
    let remain = piece_len - start;
    if remain <= 0 {
        return 0;
    }
    remain.min(i64::from(chunk_size)) as u32
}

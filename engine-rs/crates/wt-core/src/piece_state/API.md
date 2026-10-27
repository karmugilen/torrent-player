# piece_state + storage_adapter API (for agy-2)

Module: `wt_core::piece_state` — per-piece bookkeeping + dirty/requested bitmaps.
`wt_core::storage_adapter::StorageGlue` — write/read/mark/completion/error callback over `PieceStorage`.

## Types

```rust
pub struct PiecePublicState {
    pub completion: Completion, // ok, complete, err
    pub priority: PiecePriority, // effective
    pub checking: bool,          // queued_for_hash || hashing
    pub hashing: bool,
    pub queued_for_hash: bool,
    pub marking: bool,
    pub partial: bool,
    pub missing_piece_layer_hash: bool,
}

pub struct PieceStateRun {
    pub state: PiecePublicState,
    pub length: u32, // consecutive pieces with equal state
}

/// Torrent-level piece book. Caller serializes access (Go client lock).
pub struct PieceBook { /* ... */ }

/// Compact roaring bitset over u32 indices (RequestIndex / PieceIndex).
pub struct Bitset { /* roaring::RoaringBitmap */ }

/// Shared wait_change notifier (optional; torrent may keep its own).
pub struct ChangeWatch { /* AtomicU64 + Condvar */ }
```

## PieceBook signatures

```rust
impl PieceBook {
    pub fn new(num_pieces: u32, piece_length: i64, total_length: i64, chunk_size: u32) -> Self;

    // --- dirty / pending chunks (Go dirtyChunks; pending = !dirty) ---
    pub fn mark_chunk_dirty(&mut self, req: RequestIndex);      // unpend: received
    pub fn mark_chunk_pending(&mut self, req: RequestIndex);    // pend: need again
    pub fn pend_all_chunks(&mut self, piece: PieceIndex);       // clear dirty for piece
    pub fn chunk_is_dirty(&self, req: RequestIndex) -> bool;
    pub fn chunk_is_pending(&self, piece: PieceIndex, chunk_in_piece: u32) -> bool;
    pub fn piece_num_dirty_chunks(&self, piece: PieceIndex) -> u32;
    pub fn piece_all_dirty(&self, piece: PieceIndex) -> bool;
    pub fn piece_has_dirty_chunks(&self, piece: PieceIndex) -> bool;
    pub fn piece_partially_downloaded(&self, piece: PieceIndex) -> bool;
    pub fn num_dirty_bytes(&self, piece: PieceIndex) -> i64;
    pub fn iter_clean_chunk_indexes(&self, piece: PieceIndex) -> impl Iterator<Item = u32>;
    pub fn first_clean_chunk(&self, piece: PieceIndex) -> Option<u32>;

    // --- requested chunks (outstanding peer requests; compact set) ---
    pub fn mark_chunk_requested(&mut self, req: RequestIndex);
    pub fn clear_chunk_requested(&mut self, req: RequestIndex);
    pub fn chunk_is_requested(&self, req: RequestIndex) -> bool;

    // --- completion / hashing / marking ---
    pub fn update_completion(&mut self, piece: PieceIndex, c: Completion) -> bool; // returns changed
    pub fn set_complete_cached(&mut self, piece: PieceIndex, ok: bool, complete: bool) -> bool;
    pub fn piece_complete(&self, piece: PieceIndex) -> bool;
    pub fn storage_completion_ok(&self, piece: PieceIndex) -> bool;
    pub fn set_hashing(&mut self, piece: PieceIndex, v: bool);
    pub fn set_marking(&mut self, piece: PieceIndex, v: bool);
    pub fn queue_for_hash(&mut self, piece: PieceIndex);
    pub fn clear_queued_for_hash(&mut self, piece: PieceIndex);
    pub fn queued_for_hash(&self, piece: PieceIndex) -> bool;
    pub fn completed_pieces(&self) -> &Bitset;

    // --- priority (max of piece / file / reader now / reader readahead) ---
    pub fn set_piece_priority(&mut self, piece: PieceIndex, p: PiecePriority);
    pub fn set_file_priority_span(&mut self, begin: PieceIndex, end: PieceIndex, p: PiecePriority);
    pub fn set_reader_now(&mut self, pieces: &[PieceIndex]);
    pub fn set_reader_readahead(&mut self, pieces: &[PieceIndex]);
    pub fn pure_priority(&self, piece: PieceIndex) -> PiecePriority;
    pub fn effective_priority(&self, piece: PieceIndex, have_hash: bool, data_download_disallowed: bool) -> PiecePriority;
    pub fn ignore_for_requests(&self, piece: PieceIndex, have_hash: bool, data_download_disallowed: bool) -> bool;

    // --- availability (conns_with_all + relative) ---
    pub fn note_peer_has_all(&mut self);
    pub fn note_peer_lost_all(&mut self);
    pub fn increment_relative_availability(&mut self, piece: PieceIndex);
    pub fn decrement_relative_availability(&mut self, piece: PieceIndex);
    pub fn availability(&self, piece: PieceIndex) -> i32;
    pub fn conns_with_all_pieces(&self) -> i32;

    // --- public state / runs (Go PieceState / PieceStateRuns) ---
    pub fn piece_state(&self, piece: PieceIndex) -> PiecePublicState;
    pub fn piece_state_runs(&self) -> Vec<PieceStateRun>;

    // --- bytes ---
    pub fn bytes_left(&self) -> i64;
    pub fn bytes_completed(&self) -> i64;   // length - bytes_left
    pub fn bytes_missing(&self) -> i64;     // == bytes_left
    pub fn file_bytes_left(&self, file_first_piece: PieceIndex, file_end_piece: PieceIndex,
                           file_torrent_offset: i64, file_length: i64) -> i64;
    pub fn file_bytes_completed(&self, ...same...) -> i64;

    // --- geometry helpers ---
    pub fn piece_length(&self, piece: PieceIndex) -> i64;
    pub fn piece_num_chunks(&self, piece: PieceIndex) -> u32;
    pub fn piece_request_index_begin(&self, piece: PieceIndex) -> RequestIndex;
    pub fn request_index_to_piece(&self, req: RequestIndex) -> PieceIndex;

    // --- change versioning ---
    pub fn change_version(&self) -> u64;
    pub fn bump_change(&mut self) -> u64;
}

pub fn raise_priority(cur: PiecePriority, maybe: PiecePriority) -> PiecePriority;
pub fn combine_pure_priority(piece: PiecePriority, file: PiecePriority, reader_now: bool, reader_readahead: bool) -> PiecePriority;

impl ChangeWatch {
    pub fn new(initial: u64) -> Self;
    pub fn version(&self) -> u64;
    pub fn bump(&self) -> u64;
    pub fn wait_change(&self, after: u64, timeout: Duration) -> u64;
}
```

## StorageGlue signatures

```rust
pub struct StorageGlue {
    // wraps Arc<dyn TorrentStorage>
}

impl StorageGlue {
    pub fn new(storage: Arc<dyn TorrentStorage>) -> Self;
    pub fn set_on_write_chunk_error(&self, f: Option<Arc<dyn Fn(&StorageError) + Send + Sync>>);
    pub fn write_chunk(&self, piece: Piece, hash: Option<[u8; 20]>, begin: i64, data: &[u8]) -> StorageResult<()>;
    pub fn read_at(&self, piece: Piece, hash: Option<[u8; 20]>, buf: &mut [u8], off: i64) -> StorageResult<usize>;
    pub fn mark_complete(&self, piece: Piece, hash: Option<[u8; 20]>) -> StorageResult<()>;
    pub fn mark_not_complete(&self, piece: Piece, hash: Option<[u8; 20]>) -> StorageResult<()>;
    pub fn completion(&self, piece: Piece, hash: Option<[u8; 20]>) -> Completion;
    /// Refresh book from storage; returns whether cached completion changed.
    pub fn update_piece_completion(&self, book: &mut PieceBook, piece: Piece, hash: Option<[u8; 20]>) -> bool;
    pub fn piece_storage(&self, piece: Piece, hash: Option<[u8; 20]>) -> Arc<dyn PieceStorage>;
}
```

Kept from agy-2: `MemoryPieceStorage`, `MemoryTorrentStorage`, `MemoryClientStorage`, `verify_piece_hash`.

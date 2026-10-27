# piece_state + storage_adapter port notes

Owned paths: `src/piece_state/`, `src/storage_adapter.rs`, `tests/piece_state_*.rs`.
`lib.rs` only gained `pub mod piece_state;` (coordinator-approved exact edit).
agy-2 owns the rest of wt-core (`torrent.rs`, `peer_conn.rs`, crate-root `PieceState`, etc.).

## Ported

### `piece_state/`
- `PieceBook`: torrent-wide dirty/requested roaring bitsets, completed + queued-for-hash sets, per-piece flags (hashing/marking/completion cache/file+piece priority/relative availability).
- Dirty semantics match Go `dirtyChunks` (received = dirty; pending = !dirty); `pend_all_chunks` clears a piece after failed hash.
- `num_dirty_bytes` / `piece_partially_downloaded` / clean-chunk iteration match Go.
- Priority: `raise_priority` + `combine_pure_priority` (file / reader Now / reader Readahead / piece); `effective_priority` returns None when `ignore_for_requests`.
- Availability: `conns_with_all_pieces + relative_availability`.
- `PiecePublicState` / `PieceStateRun` + RLE (`piece_state_runs`) and Go `PieceStateRun.String` format.
- `bytes_left` / `bytes_completed` / `bytes_missing`; `file_bytes_left` / `file_bytes_completed` (Go `fileBytesLeft`).
- `ChangeWatch` for wait_change versioning (torrent may keep its own condvar).

### `storage_adapter.rs`
- Kept agy-2 doubles: `MemoryPieceStorage` / `MemoryTorrentStorage` / `MemoryClientStorage`, `verify_piece_hash`.
- Added `StorageGlue`: `write_chunk`, `read_at`, `mark_complete` / `mark_not_complete`, `completion`, `update_piece_completion` into `PieceBook`, `set_on_write_chunk_error`.

## Differs from Go
- Requested chunks are a roaring membership set only; peer/when stay in agy-2 `request_state` map (Go embeds weak peer pointer in `requestState`).
- Crate-root `wt_core::PieceState` (simplified, agy-2) is separate from `piece_state::PiecePublicState` (full Go fields). agy-2 should migrate callers to the latter when wiring.
- `hashing::PieceCompletionCache` remains for hasher workers; `PieceBook` is the torrent-loop source of truth for completion + flags.
- File priority spans use raise-or-exact helpers; a full file-priority rebuild should `clear_file_priorities` then set spans (Go recomputes from live `File.prio` each `purePriority` call).

## Remains (agy-2)
- Replace `TorrentInner::{pieces HashSet dirty_chunks, PieceEntry}` with `PieceBook` + `StorageGlue`.
- Wire hash pool results → `mark_*` + `update_completion` + `pend_all_chunks` on failure.
- Peer bitfield/Have → `increment_relative_availability` / `note_peer_has_all`.
- Reader position → `set_reader_now` / `set_reader_readahead`.
- Expose `piece_state_runs` on the public `Torrent` API used by `/pieces`.

## Tests
- Unit tests inside `piece_state::{bitset,priority,runs,watch}` and `storage_adapter`.
- Integration: `tests/piece_state_book.rs`, `tests/piece_state_storage.rs`.

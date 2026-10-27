# wt-storage PORT_NOTES

## Scope & Overview

Pure Rust port of Webtor engine storage (`wt-storage`) matching Go engine storage behaviour (`engine-go/storage.go`, `direct_storage.go`, `cache_store.go`, `cache_lru.go`, `checkpoint.go`, `extent.go`, `extent_host.go`, `pieces.go`, `pieces_host.go`, `stream_ranges.go`, `sparse_unix.go`, `sparse_other.go`, `verification.go`, `restore_hash.go`, `zero_hash.go`) and folding in `engine-rust/src/{hash.rs, ranges.rs, lib.rs}` logic.

Target crate: `engine-rs/crates/wt-storage/`.

Implements `wt_types::{ClientStorage, TorrentStorage, PieceStorage}`:
1. **Download Storage**: Selected files under download dir with write-back RAM piece buffer (`DEFAULT_MAX_CACHE_BYTES`), writes and reads only bytes needed (commit af9f2cb).
2. **Direct Play Storage**: Bounded RAM cache with O(1) LRU eviction, verified startup cache on disk (`.startup/<hash>/<piece>.piece` for head/tail of video files), runtime dynamic cache resizing (`SetCacheLimit` / `SetMemoryLimit`), preview budget separation.
3. **Checkpoints & Restore**: File identity tracking (`dev`, `ino`, `length`), atomic checkpoint file replacement (`.checkpoint`), self-hash skipping sparse filesystem holes, zero-hash detection, verification state.
4. **Engine Knobs & FFI Extents/Buckets**:
   - Knobs: `SetCacheLimit`, `SetMemoryLimit`, `SetRestoreVerify`.
   - File spans & extents: `FileSpan`, `hash_extents`, `pread_extents`, `webtor_hash_extents`, `webtor_pread_extents`.
   - Piece aggregation & heat grid API: `PieceRun`, `PieceRange`, `PieceBucket`, `ByteRange`, `compress_piece_runs`, `aggregate_piece_buckets`, `merge_received_range`, `ranges_cover`, `webtor_piece_buckets`, `webtor_merge_received_range`.

---

## Ported Modules

### 1. `storage.rs` ← `storage.go`
- `DocumentStorageClient`: Implements `wt_types::ClientStorage`, creating download or Direct Play `TorrentStorage` instances, managing global memory limit across all instances.
- `DocumentTorrentStorage`: Implements `wt_types::TorrentStorage`, managing piece-to-file mappings, dirty write-back buffers, stream LRU cache, and checkpoint persistence.
- `DocumentPiece`: Implements `wt_types::PieceStorage` (with inherent forwarding methods for ergonomic access without trait in scope).
- Download write-back cache:
  - Tracks dirty pieces in RAM up to `max_cache_bytes`.
  - Flushes only bytes received since last flush to disk via positional I/O (`pread`/`pwrite`).
  - Reloads evicted partial pieces transparently on subsequent access.
- Direct Play streaming RAM cache:
  - In Direct Play mode (`inner.stream = true`), pieces live in RAM tracked by `PieceLRU`.
  - Cache evictions drop unverified pieces when memory budget is exceeded.
  - Startup pieces (head/tail of streamable media files) persist to disk under `.startup/<hash>/<piece>.piece`.
  - Direct Play close skips flushing unconfigured destination disk files.

### 2. `direct_storage.rs` ← `direct_storage.go` + Android Auto RAM
- Auto RAM cache policy: `auto_ram_cache_bytes` ports `RamCachePolicy.kt` (64–512 MiB dynamic budget based on total/available RAM, system threshold, low-RAM device status, and active transfers).
- Startup cache management: `register_startup_cache`, `unregister_startup_cache`, `prune_startup_caches` enforcing maximum total `.startup` disk budget while protecting active readers.
- Direct play helpers: `open_direct_play`, `planned_startup_pieces`, `piece_fits_direct_play`.

### 3. `restore_hash.rs` ← `restore_hash.go`
- `RestoreVerifyPiece`: `PieceStorage` wrapper implementing hole-skipping verification.
- `piece_for_hash`: Returns `RestoreVerifyPiece` wrapper when `restore_verify` is enabled, otherwise plain piece storage.
- Extent hole skipping: Uses `sparse::piece_extent_all_holes` to detect unallocated blocks, returning cached `zero_piece_hash` instead of issuing disk I/O.

### 4. `checkpoint.rs` ← `checkpoint.go`
- `FileIdentity`: Tracks `(dev, ino, length)` across files in multi-file torrents.
- `PieceCheckpoint`: Binary serialization matching Go format (`version`, `files`, `completed_pieces` bitset).
- Atomic persistence: Writes to temporary file and renames to `.checkpoint`.

### 5. `cache_lru.rs` ← `cache_lru.go`
- `PieceLRU`: Intrusive doubly-linked list with HashMap providing O(1) `touch`, `push`, `remove`, and `pop_lru` operations for eviction under memory pressure.

### 6. `cache_store.rs` ← `cache_store.go`
- `PeerHintStore`: In-memory and on-disk persistent peer cache with port validation, loopback filtering, expiration, and JSON serialization.
- `DhtContactStore`: Compact binary DHT node cache (26 bytes per contact: 20-byte ID + 4-byte IPv4 + 2-byte port) with atomic file persistence.
- Resume window store: Binary persistence of verified byte ranges.
- Cache category clearing: Dynamic clearing of categories (`all`, `dht`, `peers`, `resume`).

### 7. `extent.rs` ← `extent.go`, `extent_host.go` & `engine-rust/src/hash.rs`
- `FileSpan`: `#[repr(C)]` layout matching C ABI (`fd: i32`, 4-byte padding, `offset: i64`, `length: i64`).
- `hash_extents`: Multi-extent SHA-1 hashing via Linux positional `pread`.
- `pread_extents`: Multi-extent scatter-gather reading into user buffers.
- C ABI exports: `webtor_hash_extents`, `webtor_pread_extents`.

### 8. `pieces.rs` ← `pieces.go`, `pieces_host.go` & `engine-rust/src/lib.rs`
- Heat grid aggregation:
  - `compress_piece_runs`: Compresses piece states (`Selected`, `Playing`, `Complete`, `Partial`) into run-length-encoded vectors.
  - `aggregate_piece_buckets`: Computes fixed-resolution display buckets across torrent pieces.
  - `aggregate`: Brute-force oracle verified piece counting algorithm.
  - C ABI export: `webtor_piece_buckets`.

### 9. `ranges.rs` ← `stream_ranges.go` & `engine-rust/src/ranges.rs`
- `ByteRange`: `[start, end)` byte range representation.
- `merge_received_range`: In-place sorted disjoint interval merging.
- `ranges_cover`: Checks if ranges completely cover a target interval.
- C ABI export: `webtor_merge_received_range`.

### 10. `sparse.rs` ← `sparse_unix.go`, `sparse_other.go`
- Posix/Linux file sparse queries using `libc::lseek64` (`SEEK_DATA`, `SEEK_HOLE`).
- Hole punching via `libc::fallocate` (`FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE`).
- File allocated blocks calculation via `libc::fstat`.

### 11. `verification.rs` ← `verification.go`
- `check_saved_data`: Multi-piece background verification pipeline supporting pause, cancellation, and progress updates.

### 12. `zero_hash.rs` ← `zero_hash.go`
- `zero_piece_hash`: Cached SHA-1 hashes of all-zero piece buffers of varying lengths with dynamic `LazyLock<Mutex<HashMap>>` cache.

---

## Differences & Design Decisions vs Go

1. **Memory Safety & Concurrency**:
   - In Go, `storage.go` relies on uncoordinated Mutex locks and goroutine pools. In Rust, all shared structures use `Arc<RwLock<StorageInner>>` or `Arc<Mutex<...>>` ensuring data race freedom.
   - Positional I/O (`FileExt::read_exact_at` / `write_all_at`) avoids shared file offset state across threads.
2. **Direct Play File Flushing**:
   - In Go, Direct Play does not open destination download files. In Rust, `DocumentTorrentStorage::close` checks `inner.attached && !inner.stream` before attempting to flush dirty buffers or sync checkpoints, preventing errors on unconfigured files.
3. **C ABI Layout**:
   - `FileSpan` defines `fd: i32, offset: i64, length: i64` with `#[repr(C)]`, aligning with C 24-byte layout without requiring manual padding fields.
4. **Ergonomics**:
   - `DocumentPiece` provides inherent methods (`write_at`, `read_at`, `mark_complete`, `mark_not_complete`, `completion`) mirroring `PieceStorage` trait methods, allowing direct invocation without requiring callers to import `wt_types::PieceStorage`.

---

## Acceptance & Test Results

- All tests built with: `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-storage CARGO_BUILD_JOBS=3 cargo test -p wt-storage`
- **Unit & Integration Tests**: 88 passing tests across:
  - `src/lib.rs` (23 passed)
  - `tests/port_cache_lru.rs` (3 passed)
  - `tests/port_cache_store.rs` (12 passed)
  - `tests/port_checkpoint.rs` (7 passed)
  - `tests/port_checkpoint_rel.rs` (2 passed)
  - `tests/port_direct_storage.rs` (4 passed)
  - `tests/port_extent.rs` (3 passed)
  - `tests/port_pieces.rs` (15 passed, including `brute_force_oracle`)
  - `tests/port_restore_hash.rs` (4 passed)
  - `tests/port_storage.rs` (14 passed)
  - `tests/port_stream_ranges.rs` (1 passed)
  - `tests/port_verification.rs` (5 passed)
- **Ignored tests**: Tests with explicit external test hooks (e.g. fault-injection hooks for transient write failures, or unexported mid-test inspection helpers) remain ignored as designed until external mock harnesses are wired.
- **Linters**:
  - `cargo clippy -p wt-storage --all-targets -- -D warnings`: 0 warnings, clean.
  - `cargo fmt --package wt-storage --check`: Clean, formatted.

## Accessors + test seams (2026-10-27)

Added on `DocumentTorrentStorage` / free functions for Go parity and the ported
integration suite (all previously `#[ignore]` tests now pass):

- `flush_all_dirty`, `files_overlapping`, `stream_piece_runs`
- `set_on_stream_evict`, `cache_bytes`, `max_cache_bytes`, `clear_startup_pieces`
- `replace_file_handle`, `set_file_sync_hook`, `set_write_checkpoint_hook`
- `drain_sync_and_checkpoint` (unlock-safe), `checkpoint_gen`, `flush_retry`
- Direct Play `close` skips flush when `stream` (no configured destination files)
- Async `on_stream_evict` after Direct Play `mark_complete` (close does not join)

Differs from Go: write-back backoff is driven by sync `flush_all_dirty` (no
background `scheduleDiskFlushLocked` yet).

Verify: `CARGO_BUILD_JOBS=3 cargo test -p wt-storage -- --test-threads=1`
→ 109 pass / 0 ignored / 0 fail; clippy `--all-targets -D warnings` clean.

Startup-piece persistence uses one bounded background writer and drains on close. It keeps Rust's existing file fsync; Go's startup-cache drain closes and renames without fsync. Checkpoint persistence is unchanged. The storage benchmark and its current measurement limitations are documented in BENCH.md. Unit tests `slow_startup_save_does_not_block_piece_traffic` and `slow_startup_save_does_not_block_readers` verify writers and readers continue unblocked during disk I/O.

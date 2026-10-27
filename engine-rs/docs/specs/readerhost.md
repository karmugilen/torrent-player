# ReaderHost trait (wt-core reader.rs)

Owned by reader worker. `TorrentInner` (agy-2) should implement this so `Reader` is not blocked on torrent internals. Documented at top of `reader.rs`.

```rust
/// Piece index range [begin, end) a reader wants downloaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PieceRange {
    pub begin: u32,
    pub end: u32,
}

/// Host that backs a [`Reader`]. Implemented by [`crate::torrent::TorrentInner`];
/// unit tests supply a fake. Mirrors Go `reader` ↔ `Torrent` interaction.
///
/// Methods:
/// - `have(offset, len, responsive)` — contiguous available bytes at torrent-absolute
///   `offset` (≤ `len`). Non-responsive: only completed pieces. Responsive: dirty
///   chunks count too (Go `available` / `haveChunk`).
/// - `wait_change(after, timeout)` — condvar wait until change_version > after or
///   timeout; returns current version (no busy loop).
/// - `set_reader_positions(id, range)` — register this reader's piece range so the
///   torrent recomputes readerPiecePriorities (Now = begin, Readahead = begin+1..end).
/// - `clear_reader(id)` — on Close; triggers readersChanged.
/// - `read_at(buf, torrent_offset)` — read from piece storage.
pub trait ReaderHost: Send + Sync {
    fn have(&self, offset: i64, len: i64, responsive: bool) -> i64;
    fn wait_change(&self, after: u64, timeout: std::time::Duration) -> u64;
    fn change_version(&self) -> u64;
    fn set_reader_positions(&self, reader_id: u64, range: PieceRange);
    fn clear_reader(&self, reader_id: u64);
    fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> std::io::Result<usize>;
    fn piece_length(&self) -> Option<i64>;
    fn torrent_length(&self) -> i64;
    fn num_pieces(&self) -> u32;
    fn is_closed(&self) -> bool;
    fn has_info(&self) -> bool;
    /// Webtor: called while a blocking read waits (hedge / nudge urgent).
    fn on_read_waiting(&self, torrent_offset: i64) {}
}

/// Pure Go `readerPiecePriorities`: for each [begin,end), Now={begin}, Readahead={begin+1..end}.
pub fn reader_piece_priorities(ranges: &[PieceRange]) -> (HashSet<u32>, HashSet<u32>);

/// Pure Go `byteRegionPieces(off, size)`.
pub fn byte_region_pieces(off: i64, size: i64, piece_length: i64, num_pieces: u32, torrent_length: i64) -> PieceRange;
```

## Keep lib.rs signatures
`Reader::new(Arc<TorrentInner>, file_offset, file_length)` stays. Internally stores `Arc<dyn ReaderHost>` via `TorrentInner: ReaderHost`. Also `Reader::with_host(Arc<dyn ReaderHost>, ...)` for tests.

## Ask for torrent.rs
To match Go bitmaps (`_readerNowPieces` / `_readerReadaheadPieces`), ideally TorrentInner gains:
```rust
pub reader_ranges: parking_lot::RwLock<HashMap<u64, PieceRange>>,
```
and `set_reader_positions` updates ranges + recomputes now/readahead sets used by purePriority.
Without that field I will Raise piece.priority (Now / Readahead) from the ReaderHost impl in reader.rs as a stopgap — priorities won't drop on seek-away until agy-2 wires the bitmaps. Prefer the field if you can add it (or approve a one-line ask to agy-2).
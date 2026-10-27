//! Seekable, responsive torrent reader (Go `reader.go` + Webtor responsive).
//!
//! # `ReaderHost`
//!
//! The reader talks to the torrent only through [`ReaderHost`], so this module
//! is not blocked on `torrent.rs` internals. `TorrentInner` implements the
//! trait; unit tests supply a fake.
//!
//! ```ignore
//! /// Piece index range [begin, end) a reader wants downloaded.
//! pub struct PieceRange { pub begin: u32, pub end: u32 }
//!
//! /// Host that backs a [`Reader`].
//! ///
//! /// - `have(offset, len, responsive)` — contiguous available bytes at
//! ///   torrent-absolute `offset` (≤ `len`). Non-responsive: completed pieces
//! ///   only. Responsive: dirty chunks count too (Go `available` / `haveChunk`).
//! /// - `wait_change(after, timeout)` — condvar wait until change_version >
//! ///   `after` or timeout; returns current version (no busy loop).
//! /// - `set_reader_positions(id, range)` — register this reader's piece range
//! ///   so priorities recompute like Go `readerPiecePriorities`
//! ///   (Now = begin, Readahead = begin+1..end).
//! /// - `clear_reader(id)` — on Close; Go `deleteReader` / `readersChanged`.
//! /// - `read_at(buf, torrent_offset)` — read from piece storage.
//! pub trait ReaderHost: Send + Sync {
//!     fn have(&self, offset: i64, len: i64, responsive: bool) -> i64;
//!     fn wait_change(&self, after: u64, timeout: Duration) -> u64;
//!     fn change_version(&self) -> u64;
//!     fn set_reader_positions(&self, reader_id: u64, range: PieceRange);
//!     fn clear_reader(&self, reader_id: u64);
//!     fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> io::Result<usize>;
//!     fn piece_length(&self) -> Option<i64>;
//!     fn torrent_length(&self) -> i64;
//!     fn num_pieces(&self) -> u32;
//!     fn is_closed(&self) -> bool;
//!     fn has_info(&self) -> bool;
//!     fn on_read_waiting(&self, torrent_offset: i64) {}
//! }
//! ```
//!
//! Stopgap: `raise_reader_piece_priorities_stopgap` Raises `piece.priority`
//! to Now/Readahead (priorities do not drop on seek-away). Full Go bitmaps
//! (`_readerNowPieces` / `_readerReadaheadPieces`) belong on the torrent once
//! agy-2 adds `reader_ranges` storage; switch by replacing that one function.

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wt_types::{PiecePriority, BLOCK_SIZE};

use crate::picker::req_index_from_piece_and_offset;
use crate::torrent::TorrentInner;
use crate::{CancelToken, ReadaheadContext, ReadaheadFn};

/// Piece index range `[begin, end)` a reader wants downloaded (Go `pieceRange`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PieceRange {
    pub begin: u32,
    pub end: u32,
}

impl PieceRange {
    pub fn is_empty(self) -> bool {
        self.begin >= self.end
    }
}

/// Host that backs a [`Reader`]. Implemented by [`TorrentInner`]; tests use a fake.
pub trait ReaderHost: Send + Sync {
    /// Contiguous available bytes at torrent-absolute `offset`, capped by `len`.
    fn have(&self, offset: i64, len: i64, responsive: bool) -> i64;

    /// Wait until `change_version > after` or `timeout`. Returns current version.
    fn wait_change(&self, after: u64, timeout: Duration) -> u64;

    fn change_version(&self) -> u64;

    /// Register this reader's desired piece range (Go `readerPosChanged`).
    fn set_reader_positions(&self, reader_id: u64, range: PieceRange);

    /// Unregister reader on close (Go `deleteReader`).
    fn clear_reader(&self, reader_id: u64);

    /// Read from piece storage at torrent-absolute offset.
    fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> std::io::Result<usize>;

    fn piece_length(&self) -> Option<i64>;
    fn torrent_length(&self) -> i64;
    fn num_pieces(&self) -> u32;
    fn is_closed(&self) -> bool;
    fn has_info(&self) -> bool;

    /// Webtor: called while a blocking read waits (hedge / nudge urgent).
    fn on_read_waiting(&self, _torrent_offset: i64) {}
}

/// Pure Go `byteRegionPieces(off, size)` → `[begin, end)`.
pub fn byte_region_pieces(
    off: i64,
    size: i64,
    piece_length: i64,
    num_pieces: u32,
    torrent_length: i64,
) -> PieceRange {
    if piece_length <= 0 || off >= torrent_length {
        return PieceRange::default();
    }
    let mut off = off;
    let mut size = size;
    if off < 0 {
        size += off;
        off = 0;
    }
    if size <= 0 {
        return PieceRange::default();
    }
    let begin = (off / piece_length) as u32;
    let mut end = ((off + size + piece_length - 1) / piece_length) as u32;
    if end > num_pieces {
        end = num_pieces;
    }
    PieceRange { begin, end }
}

/// Pure Go `readerPiecePriorities`: for each `[begin,end)`,
/// Now = `{begin}`, Readahead = `{begin+1 .. end}`.
pub fn reader_piece_priorities(ranges: &[PieceRange]) -> (HashSet<u32>, HashSet<u32>) {
    let mut now = HashSet::new();
    let mut readahead = HashSet::new();
    for r in ranges {
        if r.end > r.begin {
            now.insert(r.begin);
            for i in (r.begin + 1)..r.end {
                readahead.insert(i);
            }
        }
    }
    (now, readahead)
}

/// Go `defaultReadaheadFunc`: grows with contiguous read progress.
pub fn default_readahead_fn(ctx: &ReadaheadContext) -> i64 {
    ctx.current_pos - ctx.contiguous_read_start_pos
}

static NEXT_READER_ID: AtomicU64 = AtomicU64::new(1);

/// Seekable reader over torrent (or file) bytes. Not safe for concurrent use.
pub struct Reader {
    host: Arc<dyn ReaderHost>,
    reader_id: u64,
    /// Torrent-absolute start of this reader's window (0 for whole torrent).
    pub file_offset: i64,
    /// Length of the window (file length or torrent length).
    pub file_length: i64,
    pub pos: i64,
    pub contiguous_start: i64,
    /// Static readahead when `readahead_fn` is None.
    pub readahead: i64,
    pub readahead_fn: Option<ReadaheadFn>,
    /// Reads have started since last seek (Go `reading`).
    reading: bool,
    pub responsive: bool,
    pub cancel: Option<CancelToken>,
    /// Cached piece range last pushed to the host.
    pieces: PieceRange,
    closed: bool,
}

impl Reader {
    /// Construct bound to a torrent (keeps `lib.rs` / `File::new_reader` signature).
    pub fn new(torrent: Arc<TorrentInner>, file_offset: i64, file_length: i64) -> Self {
        Self::with_host(torrent, file_offset, file_length)
    }

    /// Construct against any [`ReaderHost`] (unit tests).
    pub fn with_host(host: Arc<dyn ReaderHost>, file_offset: i64, file_length: i64) -> Self {
        let reader_id = NEXT_READER_ID.fetch_add(1, Ordering::Relaxed);
        let mut r = Self {
            host,
            reader_id,
            file_offset,
            file_length,
            pos: 0,
            contiguous_start: 0,
            readahead: 0,
            // Go `newReader` sets `readaheadFunc = defaultReadaheadFunc`.
            readahead_fn: Some(Arc::new(default_readahead_fn)),
            reading: false,
            responsive: false,
            cancel: None,
            pieces: PieceRange::default(),
            closed: false,
        };
        r.pos_changed();
        r
    }

    pub fn set_readahead(&mut self, bytes: i64) {
        self.readahead = bytes.max(0);
        self.readahead_fn = None;
        self.pos_changed();
    }

    pub fn set_readahead_fn(&mut self, f: Option<ReadaheadFn>) {
        self.readahead_fn = f;
        self.pos_changed();
    }

    /// Go `SetResponsive` — sticky on (cannot undo via this method).
    pub fn set_responsive(&mut self) {
        self.responsive = true;
        // Wake waiters (Go broadcasts client event via set_reader_positions → bump).
        self.host.set_reader_positions(self.reader_id, self.pieces);
    }

    /// Go `WebtorSetResponsive` — can turn responsive mode on or off.
    pub fn set_webtor_responsive(&mut self, on: bool) {
        if self.responsive != on {
            self.responsive = on;
            self.host.set_reader_positions(self.reader_id, self.pieces);
        }
    }

    pub fn set_cancel(&mut self, cancel: CancelToken) {
        self.cancel = Some(cancel);
    }

    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.host.clear_reader(self.reader_id);
        }
    }

    fn torrent_offset(&self, reader_pos: i64) -> i64 {
        self.file_offset + reader_pos
    }

    fn check_cancel_closed(&self) -> std::io::Result<()> {
        if self.closed || self.host.is_closed() {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "torrent closed",
            ));
        }
        if let Some(ref c) = self.cancel {
            if c.is_cancelled() {
                return Err(std::io::Error::new(ErrorKind::Interrupted, "cancelled"));
            }
        }
        Ok(())
    }

    /// Desired piece range (Go `piecesUncached`).
    pub fn pieces_uncached(&self) -> PieceRange {
        let Some(piece_len) = self.host.piece_length() else {
            return PieceRange::default();
        };
        let mut ra = self.readahead;
        if let Some(ref f) = self.readahead_fn {
            ra = f(&ReadaheadContext {
                current_pos: self.pos,
                contiguous_read_start_pos: self.contiguous_start,
            });
        }
        if ra < 1 {
            // Needs to be at least 1: [x, x) means we want nothing.
            ra = 1;
        }
        if !self.reading {
            ra = 0;
        }
        if ra > self.file_length - self.pos {
            ra = self.file_length - self.pos;
        }
        byte_region_pieces(
            self.torrent_offset(self.pos),
            ra,
            piece_len,
            self.host.num_pieces(),
            self.host.torrent_length(),
        )
    }

    fn pos_changed(&mut self) {
        let to = self.pieces_uncached();
        if to == self.pieces {
            return;
        }
        self.pieces = to;
        self.host.set_reader_positions(self.reader_id, to);
    }

    /// Blocking read with optional deadline (Go `readContext` + timeout).
    pub fn read_timeout(
        &mut self,
        buf: &mut [u8],
        timeout: Option<Duration>,
    ) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.check_cancel_closed()?;
        if self.pos >= self.file_length {
            return Ok(0); // EOF
        }

        let deadline = timeout.map(|d| Instant::now() + d);

        // Wait for info if needed.
        while !self.host.has_info() {
            self.check_cancel_closed()?;
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "timeout waiting for info",
                    ));
                }
            }
            let after = self.host.change_version();
            let wait = deadline
                .map(|dl| dl.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(60));
            self.host.wait_change(after, wait);
        }

        // Mark reading and refresh piece priorities (Go sets reading before readAt).
        self.reading = true;
        self.pos_changed();

        let n = self.read_at_pos(buf, self.pos, deadline)?;
        if n == 0 {
            // At EOF or nothing available without error.
            if self.pos >= self.file_length {
                return Ok(0);
            }
            // Should have errored if buf was non-empty and not EOF.
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "no data available",
            ));
        }

        self.pos += n as i64;
        self.pos_changed();
        Ok(n)
    }

    /// Wait until data is available, then perform one storage read (Go `readOnceAt`).
    fn read_at_pos(
        &mut self,
        buf: &mut [u8],
        pos: i64,
        deadline: Option<Instant>,
    ) -> std::io::Result<usize> {
        if pos >= self.file_length {
            return Ok(0);
        }
        let wanted = (buf.len() as i64).min(self.file_length - pos);

        loop {
            self.check_cancel_closed()?;

            let torrent_off = self.torrent_offset(pos);
            let avail = self.host.have(torrent_off, wanted, self.responsive);
            if avail > 0 {
                let to_read = (avail as usize).min(buf.len());
                match self.host.read_at(&mut buf[..to_read], torrent_off) {
                    Ok(0) => {
                        // Storage said available but read 0 — retry after wait.
                    }
                    Ok(n) => return Ok(n),
                    Err(e) => {
                        // Retry once after a change (Go resets storage reader).
                        self.host.on_read_waiting(torrent_off);
                        let after = self.host.change_version();
                        let wait = remaining(deadline).unwrap_or(Duration::from_millis(50));
                        self.host
                            .wait_change(after, wait.min(Duration::from_millis(50)));
                        self.check_cancel_closed()?;
                        let avail2 = self.host.have(torrent_off, wanted, self.responsive);
                        if avail2 > 0 {
                            let to_read2 = (avail2 as usize).min(buf.len());
                            return self.host.read_at(&mut buf[..to_read2], torrent_off);
                        }
                        return Err(e);
                    }
                }
            }

            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    return Err(std::io::Error::new(ErrorKind::TimedOut, "read timed out"));
                }
            }

            self.host.on_read_waiting(torrent_off);
            let after = self.host.change_version();
            let wait = remaining(deadline).unwrap_or(Duration::from_secs(60));
            self.host.wait_change(after, wait);
        }
    }
}

fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|dl| dl.saturating_duration_since(Instant::now()))
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.close();
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read_timeout(buf, None)
    }
}

impl Seek for Reader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(off) => off as i64,
            SeekFrom::Current(off) => self.pos + off,
            SeekFrom::End(off) => self.file_length + off,
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "negative seek position",
            ));
        }
        if new_pos != self.pos {
            self.reading = false;
            self.pos = new_pos;
            self.contiguous_start = new_pos;
            self.pos_changed();
        }
        Ok(self.pos as u64)
    }
}

// -------------------------------------------------------------------------------------------------
// TorrentInner as ReaderHost
// -------------------------------------------------------------------------------------------------

impl ReaderHost for TorrentInner {
    fn have(&self, offset: i64, len: i64, responsive: bool) -> i64 {
        let Some(info) = self.info.read().clone() else {
            return 0;
        };
        let piece_len = info.piece_length;
        if piece_len <= 0 {
            return 0;
        }
        let total = info.total_length();
        let mut off = offset;
        let mut max = len;
        let mut ret = 0i64;
        let pieces = self.pieces.read();
        let dirty = self.dirty_chunks.read();

        while max > 0 {
            if off < 0 || off >= total {
                break;
            }
            let piece = (off / piece_len) as usize;
            if piece >= pieces.len() {
                break;
            }
            let in_piece = off % piece_len;
            let chunk_start = (in_piece / BLOCK_SIZE as i64) * BLOCK_SIZE as i64;
            let piece_end = ((piece as i64 + 1) * piece_len).min(total);
            let piece_base = piece as i64 * piece_len;
            let chunk_len = (piece_end - (piece_base + chunk_start)).min(BLOCK_SIZE as i64);
            if chunk_len <= 0 {
                break;
            }

            let complete = pieces[piece].complete;
            if !responsive && !complete {
                break;
            }
            if !complete {
                let req =
                    req_index_from_piece_and_offset(piece as u32, chunk_start as u32, piece_len);
                if !dirty.contains(&req) {
                    break;
                }
            }

            let len1 = chunk_len - (in_piece - chunk_start);
            if len1 <= 0 {
                break;
            }
            max -= len1;
            ret += len1;
            off += len1;
        }
        if max < 0 {
            ret += max;
        }
        ret
    }

    fn wait_change(&self, after: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        let mut guard = self.change_mutex.lock();
        loop {
            let cur = self.change_version.load(Ordering::SeqCst);
            if cur > after || Instant::now() >= deadline {
                return cur;
            }
            let wait_dur = deadline.saturating_duration_since(Instant::now());
            if wait_dur.is_zero() {
                return cur;
            }
            self.change_cond.wait_for(&mut guard, wait_dur);
        }
    }

    fn change_version(&self) -> u64 {
        self.change_version.load(Ordering::SeqCst)
    }

    fn set_reader_positions(&self, _reader_id: u64, range: PieceRange) {
        raise_reader_piece_priorities_stopgap(self, range);
        self.bump_change();
    }

    fn clear_reader(&self, _reader_id: u64) {
        self.bump_change();
    }

    fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> std::io::Result<usize> {
        let info = self.info.read().clone().ok_or_else(|| {
            std::io::Error::new(ErrorKind::NotConnected, "torrent info not available")
        })?;
        let storage = self.storage.read().clone().ok_or_else(|| {
            std::io::Error::new(ErrorKind::NotConnected, "torrent storage not available")
        })?;
        let piece_len = info.piece_length;
        if piece_len <= 0 || torrent_offset < 0 || torrent_offset >= info.total_length() {
            return Ok(0);
        }
        let piece_idx = (torrent_offset / piece_len) as u32;
        let in_piece = torrent_offset % piece_len;
        let piece_obj = info.piece(piece_idx);
        let rem = (piece_obj.length - in_piece).max(0) as usize;
        if rem == 0 || buf.is_empty() {
            return Ok(0);
        }
        let to_read = buf.len().min(rem);
        let ps = storage.piece(piece_obj, info.piece_hash_v1(piece_idx));
        ps.read_at(&mut buf[..to_read], in_piece)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    fn piece_length(&self) -> Option<i64> {
        self.info.read().as_ref().map(|i| i.piece_length)
    }

    fn torrent_length(&self) -> i64 {
        self.info
            .read()
            .as_ref()
            .map(|i| i.total_length())
            .unwrap_or(0)
    }

    fn num_pieces(&self) -> u32 {
        self.info
            .read()
            .as_ref()
            .map(|i| i.num_pieces())
            .unwrap_or(0)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn has_info(&self) -> bool {
        self.info.read().is_some()
    }

    fn on_read_waiting(&self, torrent_offset: i64) {
        self.request_focus
            .store(torrent_offset + 1, Ordering::Relaxed);
        self.nudge_urgent_requests();
    }
}

/// STOPGAP: Raise `piece.priority` to Now/Readahead for a reader range.
///
/// Replace with bitmap-based `reader_ranges` → `readerPiecePriorities` once
/// agy-2 adds that field on `TorrentInner`. Keep all Raise logic here so the
/// switch is a one-function change.
fn raise_reader_piece_priorities_stopgap(t: &TorrentInner, range: PieceRange) {
    if range.is_empty() {
        return;
    }
    let mut pieces = t.pieces.write();
    if let Some(p) = pieces.get_mut(range.begin as usize) {
        if p.priority < PiecePriority::Now {
            p.priority = PiecePriority::Now;
        }
    }
    for i in (range.begin + 1)..range.end {
        if let Some(p) = pieces.get_mut(i as usize) {
            if p.priority < PiecePriority::Readahead {
                p.priority = PiecePriority::Readahead;
            }
        }
    }
    drop(pieces);

    if let Some(piece_len) = t.info.read().as_ref().map(|i| i.piece_length) {
        let focus_off = range.begin as i64 * piece_len;
        t.request_focus.store(focus_off + 1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn byte_region_pieces_basic() {
        // 1 MiB pieces, 4 pieces total (4 MiB torrent)
        let r = byte_region_pieces(0, 1, 1 << 20, 4, 4 << 20);
        assert_eq!(r, PieceRange { begin: 0, end: 1 });

        let r = byte_region_pieces(0, 1 << 20, 1 << 20, 4, 4 << 20);
        assert_eq!(r, PieceRange { begin: 0, end: 1 });

        let r = byte_region_pieces(0, (1 << 20) + 1, 1 << 20, 4, 4 << 20);
        assert_eq!(r, PieceRange { begin: 0, end: 2 });

        let r = byte_region_pieces(1 << 20, 2 << 20, 1 << 20, 4, 4 << 20);
        assert_eq!(r, PieceRange { begin: 1, end: 3 });
    }

    #[test]
    fn byte_region_pieces_empty_and_clamp() {
        assert!(byte_region_pieces(0, 0, 1 << 20, 4, 4 << 20).is_empty());
        assert!(byte_region_pieces(4 << 20, 100, 1 << 20, 4, 4 << 20).is_empty());
        let r = byte_region_pieces(3 << 20, 2 << 20, 1 << 20, 4, 4 << 20);
        assert_eq!(r, PieceRange { begin: 3, end: 4 });
    }

    #[test]
    fn reader_piece_priorities_matches_go() {
        // Single reader wanting pieces [2, 6): Now={2}, Readahead={3,4,5}
        let (now, ra) = reader_piece_priorities(&[PieceRange { begin: 2, end: 6 }]);
        assert_eq!(now, HashSet::from([2]));
        assert_eq!(ra, HashSet::from([3, 4, 5]));

        // Two readers
        let (now, ra) = reader_piece_priorities(&[
            PieceRange { begin: 0, end: 3 },
            PieceRange { begin: 10, end: 12 },
        ]);
        assert_eq!(now, HashSet::from([0, 10]));
        assert_eq!(ra, HashSet::from([1, 2, 11]));

        // Empty range ignored
        let (now, ra) = reader_piece_priorities(&[PieceRange { begin: 5, end: 5 }]);
        assert!(now.is_empty());
        assert!(ra.is_empty());
    }
}

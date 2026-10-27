//! Reader unit tests against a fake [`ReaderHost`] (no full torrent required).
//!
//! Covers: blocking until data arrives, seek into missing data, readahead
//! priority marking (vs Go `readerPiecePriorities`), responsive mode, cancel,
//! EOF at file end, file-relative offsets.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};
use wt_core::reader::{reader_piece_priorities, PieceRange, Reader, ReaderHost};
use wt_core::{CancelToken, ReadaheadContext, BLOCK_SIZE};

// -------------------------------------------------------------------------------------------------
// Fake host
// -------------------------------------------------------------------------------------------------

struct FakeHostInner {
    data: Vec<u8>,
    /// Completed piece indexes (non-responsive reads need these).
    complete: HashSet<u32>,
    /// Dirty request indexes (responsive reads).
    dirty: HashSet<u32>,
    piece_length: i64,
    closed: bool,
    change_version: u64,
    reader_ranges: HashMap<u64, PieceRange>,
    /// Last on_read_waiting offset (for assertions).
    last_wait_offset: Option<i64>,
}

struct FakeHost {
    inner: Mutex<FakeHostInner>,
    cond: Condvar,
    has_info: AtomicBool,
    change_version_atomic: AtomicU64,
}

impl FakeHost {
    fn new(data: Vec<u8>, piece_length: i64) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(FakeHostInner {
                data,
                complete: HashSet::new(),
                dirty: HashSet::new(),
                piece_length,
                closed: false,
                change_version: 1,
                reader_ranges: HashMap::new(),
                last_wait_offset: None,
            }),
            cond: Condvar::new(),
            has_info: AtomicBool::new(true),
            change_version_atomic: AtomicU64::new(1),
        })
    }

    fn bump(self: &Arc<Self>) {
        let mut g = self.inner.lock();
        g.change_version += 1;
        self.change_version_atomic
            .store(g.change_version, Ordering::SeqCst);
        self.cond.notify_all();
    }

    fn mark_complete(self: &Arc<Self>, piece: u32) {
        self.inner.lock().complete.insert(piece);
        self.bump();
    }

    fn mark_dirty_block(self: &Arc<Self>, torrent_offset: i64) {
        let req = (torrent_offset / BLOCK_SIZE as i64) as u32;
        self.inner.lock().dirty.insert(req);
        self.bump();
    }

    fn write_bytes(self: &Arc<Self>, offset: usize, bytes: &[u8]) {
        let mut g = self.inner.lock();
        let end = (offset + bytes.len()).min(g.data.len());
        let n = end - offset;
        g.data[offset..end].copy_from_slice(&bytes[..n]);
    }

    fn reader_ranges(&self) -> Vec<PieceRange> {
        self.inner.lock().reader_ranges.values().copied().collect()
    }

    fn priorities(&self) -> (HashSet<u32>, HashSet<u32>) {
        let ranges = self.reader_ranges();
        reader_piece_priorities(&ranges)
    }
}

impl ReaderHost for FakeHost {
    fn have(&self, offset: i64, len: i64, responsive: bool) -> i64 {
        let g = self.inner.lock();
        let piece_len = g.piece_length;
        let total = g.data.len() as i64;
        let mut off = offset;
        let mut max = len;
        let mut ret = 0i64;
        while max > 0 {
            if off < 0 || off >= total {
                break;
            }
            let piece = (off / piece_len) as u32;
            let in_piece = off % piece_len;
            let chunk_start = (in_piece / BLOCK_SIZE as i64) * BLOCK_SIZE as i64;
            let piece_end = ((piece as i64 + 1) * piece_len).min(total);
            let piece_base = piece as i64 * piece_len;
            let chunk_len = (piece_end - (piece_base + chunk_start)).min(BLOCK_SIZE as i64);
            if chunk_len <= 0 {
                break;
            }
            let complete = g.complete.contains(&piece);
            if !responsive && !complete {
                break;
            }
            if !complete {
                let req = ((piece_base + chunk_start) / BLOCK_SIZE as i64) as u32;
                if !g.dirty.contains(&req) {
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
        let deadline = std::time::Instant::now() + timeout;
        let mut g = self.inner.lock();
        loop {
            if g.change_version > after || std::time::Instant::now() >= deadline {
                return g.change_version;
            }
            let wait = deadline.saturating_duration_since(std::time::Instant::now());
            if wait.is_zero() {
                return g.change_version;
            }
            self.cond.wait_for(&mut g, wait);
        }
    }

    fn change_version(&self) -> u64 {
        self.change_version_atomic.load(Ordering::SeqCst)
    }

    fn set_reader_positions(&self, reader_id: u64, range: PieceRange) {
        let mut g = self.inner.lock();
        g.reader_ranges.insert(reader_id, range);
        g.change_version += 1;
        self.change_version_atomic
            .store(g.change_version, Ordering::SeqCst);
        self.cond.notify_all();
    }

    fn clear_reader(&self, reader_id: u64) {
        let mut g = self.inner.lock();
        g.reader_ranges.remove(&reader_id);
        g.change_version += 1;
        self.change_version_atomic
            .store(g.change_version, Ordering::SeqCst);
        self.cond.notify_all();
    }

    fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> std::io::Result<usize> {
        let g = self.inner.lock();
        if torrent_offset < 0 {
            return Ok(0);
        }
        let off = torrent_offset as usize;
        if off >= g.data.len() || buf.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(g.data.len() - off);
        buf[..n].copy_from_slice(&g.data[off..off + n]);
        Ok(n)
    }

    fn piece_length(&self) -> Option<i64> {
        Some(self.inner.lock().piece_length)
    }

    fn torrent_length(&self) -> i64 {
        self.inner.lock().data.len() as i64
    }

    fn num_pieces(&self) -> u32 {
        let g = self.inner.lock();
        ((g.data.len() as i64 + g.piece_length - 1) / g.piece_length) as u32
    }

    fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    fn has_info(&self) -> bool {
        self.has_info.load(Ordering::Relaxed)
    }

    fn on_read_waiting(&self, torrent_offset: i64) {
        self.inner.lock().last_wait_offset = Some(torrent_offset);
    }
}

// -------------------------------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------------------------------

#[test]
fn blocking_read_waits_until_data_arrives() {
    let piece_len = 256 * 1024i64;
    let host = FakeHost::new(vec![0u8; (piece_len * 2) as usize], piece_len);
    let host_bg = host.clone();
    let mut reader = Reader::with_host(host.clone(), 0, host.torrent_length());

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        host_bg.write_bytes(0, &[0x5a; 1024]);
        host_bg.mark_complete(0);
    });

    let mut buf = vec![0u8; 100];
    let n = reader
        .read_timeout(&mut buf, Some(Duration::from_secs(2)))
        .expect("read should succeed once data arrives");
    assert_eq!(n, 100);
    assert!(buf.iter().all(|&b| b == 0x5a));
}

#[test]
fn seek_into_missing_data_then_blocks() {
    let piece_len = 256 * 1024i64;
    let host = FakeHost::new(vec![0u8; (piece_len * 4) as usize], piece_len);
    // Piece 0 present; piece 1 missing.
    host.write_bytes(0, &vec![0x11; piece_len as usize]);
    host.mark_complete(0);

    let mut reader = Reader::with_host(host.clone(), 0, host.torrent_length());
    reader.seek(SeekFrom::Start(piece_len as u64 + 50)).unwrap();

    let host_bg = host.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let mut pattern = vec![0u8; piece_len as usize];
        pattern[50..150].fill(0x42);
        host_bg.write_bytes(piece_len as usize, &pattern);
        host_bg.mark_complete(1);
    });

    let mut buf = vec![0u8; 100];
    let n = reader
        .read_timeout(&mut buf, Some(Duration::from_secs(2)))
        .unwrap();
    assert_eq!(n, 100);
    assert_eq!(buf, vec![0x42; 100]);
}

#[test]
fn readahead_priority_marking_matches_go() {
    let piece_len = 256 * 1024i64;
    let total = piece_len * 8;
    let host = FakeHost::new(vec![0u8; total as usize], piece_len);
    // Fill everything so reads don't block; we care about priorities.
    for p in 0..8u32 {
        host.mark_complete(p);
    }

    let mut reader = Reader::with_host(host.clone(), 0, total);
    // Static 3-piece readahead window from pos 0 after first read starts.
    reader.set_readahead(piece_len * 3);

    let mut buf = vec![0u8; 64];
    assert_eq!(reader.read(&mut buf).unwrap(), 64);

    assert_eq!(host.reader_ranges().len(), 1);
    // Position advanced; priorities must match Go readerPiecePriorities for the
    // reader's current uncached range.
    let current = reader.pieces_uncached();
    let (now, ra) = host.priorities();
    let (want_now, want_ra) = reader_piece_priorities(&[current]);
    assert_eq!(
        now, want_now,
        "Now pieces must match Go readerPiecePriorities"
    );
    assert_eq!(
        ra, want_ra,
        "Readahead pieces must match Go readerPiecePriorities"
    );
    assert!(now.contains(&0), "current piece is Now: {now:?}");
    assert!(
        ra.contains(&1) && ra.contains(&2),
        "readahead window: {ra:?}"
    );
}

#[test]
fn readahead_func_grows_with_contiguous_read() {
    let piece_len = 64 * 1024i64;
    let total = piece_len * 16;
    let host = FakeHost::new(vec![0xab; total as usize], piece_len);
    for p in 0..16u32 {
        host.mark_complete(p);
    }

    let mut reader = Reader::with_host(host.clone(), 0, total);
    // Default readahead_fn = current_pos - contiguous_start.
    let mut buf = vec![0u8; (piece_len * 2) as usize];
    assert_eq!(reader.read(&mut buf).unwrap(), buf.len());
    // After reading 2 pieces from 0: pos = 2*piece_len, contiguous_start = 0,
    // default ra = pos - start = 2*piece_len → byteRegionPieces → [2, 4).
    // Go: Now = {begin=2}, Readahead = {3}.
    let current = reader.pieces_uncached();
    let (now, ra) = host.priorities();
    let (want_now, want_ra) = reader_piece_priorities(&[current]);
    assert_eq!(now, want_now);
    assert_eq!(ra, want_ra);
    assert_eq!(now, HashSet::from([2]));
    assert!(ra.contains(&3), "readahead should include piece 3: {ra:?}");
}

#[test]
fn responsive_mode_returns_dirty_chunk_before_piece_complete() {
    let piece_len = 256 * 1024i64;
    let host = FakeHost::new(vec![0u8; piece_len as usize], piece_len);
    host.write_bytes(0, &[0x77; BLOCK_SIZE as usize]);
    host.mark_dirty_block(0);

    let mut reader = Reader::with_host(host.clone(), 0, piece_len);
    reader.set_responsive();

    let mut buf = vec![0u8; 1024];
    let n = reader
        .read_timeout(&mut buf, Some(Duration::from_millis(200)))
        .unwrap();
    assert_eq!(n, 1024);
    assert_eq!(buf, vec![0x77; 1024]);
}

#[test]
fn non_responsive_ignores_dirty_only_chunk() {
    let piece_len = 256 * 1024i64;
    let host = FakeHost::new(vec![0u8; piece_len as usize], piece_len);
    host.write_bytes(0, &[0x77; BLOCK_SIZE as usize]);
    host.mark_dirty_block(0);

    let mut reader = Reader::with_host(host, 0, piece_len);
    // responsive = false
    let mut buf = vec![0u8; 1024];
    let err = reader
        .read_timeout(&mut buf, Some(Duration::from_millis(80)))
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn cancel_returns_interrupted() {
    let piece_len = 256 * 1024i64;
    let host = FakeHost::new(vec![0u8; piece_len as usize], piece_len);
    let mut reader = Reader::with_host(host, 0, piece_len);
    let cancel = CancelToken::default();
    cancel.cancel();
    reader.set_cancel(cancel);

    let mut buf = vec![0u8; 1024];
    let err = reader
        .read_timeout(&mut buf, Some(Duration::from_millis(50)))
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
}

#[test]
fn eof_at_file_end() {
    let piece_len = 64 * 1024i64;
    let file_len = 1000i64;
    let host = FakeHost::new(vec![0xcd; piece_len as usize], piece_len);
    host.mark_complete(0);

    // File-bounded reader shorter than torrent.
    let mut reader = Reader::with_host(host, 0, file_len);
    let mut buf = vec![0u8; 2000];
    let n = reader.read(&mut buf).unwrap();
    assert_eq!(n, 1000);
    assert_eq!(&buf[..1000], &vec![0xcd; 1000]);

    let n2 = reader.read(&mut buf).unwrap();
    assert_eq!(n2, 0, "EOF");
}

#[test]
fn file_relative_offsets() {
    let piece_len = 64 * 1024i64;
    let mut data = vec![0u8; (piece_len * 3) as usize];
    // File starts at torrent offset piece_len, length = piece_len.
    data[piece_len as usize..(piece_len as usize + 64)].fill(0xef);
    let host = FakeHost::new(data, piece_len);
    host.mark_complete(1);

    let mut reader = Reader::with_host(host, piece_len, piece_len);
    let mut buf = vec![0u8; 64];
    let n = reader.read(&mut buf).unwrap();
    assert_eq!(n, 64);
    assert_eq!(buf, vec![0xef; 64]);

    // Seek within file.
    reader.seek(SeekFrom::Start(0)).unwrap();
    assert_eq!(reader.pos, 0);
}

#[test]
fn webtor_set_responsive_toggle() {
    let piece_len = 256 * 1024i64;
    let host = FakeHost::new(vec![0u8; piece_len as usize], piece_len);
    host.write_bytes(0, &[0x99; BLOCK_SIZE as usize]);
    host.mark_dirty_block(0);

    let mut reader = Reader::with_host(host, 0, piece_len);
    reader.set_webtor_responsive(true);
    assert!(reader.responsive);

    let mut buf = vec![0u8; 16];
    assert_eq!(
        reader
            .read_timeout(&mut buf, Some(Duration::from_millis(100)))
            .unwrap(),
        16
    );

    reader.set_webtor_responsive(false);
    assert!(!reader.responsive);
}

#[test]
fn go_reader_piece_priorities_vectors() {
    // Direct comparison vectors for the pure function (same inputs as Go).
    let cases: &[(&[PieceRange], &[u32], &[u32])] = &[
        (&[PieceRange { begin: 0, end: 1 }], &[0], &[]),
        (&[PieceRange { begin: 0, end: 4 }], &[0], &[1, 2, 3]),
        (
            &[
                PieceRange { begin: 5, end: 8 },
                PieceRange { begin: 0, end: 2 },
            ],
            &[5, 0],
            &[6, 7, 1],
        ),
        (&[PieceRange { begin: 3, end: 3 }], &[], &[]),
    ];
    for (ranges, want_now, want_ra) in cases {
        let (now, ra) = reader_piece_priorities(ranges);
        assert_eq!(now, want_now.iter().copied().collect::<HashSet<_>>());
        assert_eq!(ra, want_ra.iter().copied().collect::<HashSet<_>>());
    }
}

#[test]
fn set_readahead_fn_context() {
    let piece_len = 64 * 1024i64;
    let total = piece_len * 8;
    let host = FakeHost::new(vec![0u8; total as usize], piece_len);
    for p in 0..8 {
        host.mark_complete(p);
    }

    let seen = Arc::new(RwLock::new(None::<ReadaheadContext>));
    let seen2 = seen.clone();
    let mut reader = Reader::with_host(host, 0, total);
    reader.set_readahead_fn(Some(Arc::new(move |ctx: &ReadaheadContext| {
        *seen2.write() = Some(*ctx);
        piece_len * 2
    })));

    reader.seek(SeekFrom::Start(100)).unwrap();
    let mut buf = vec![0u8; 8];
    assert_eq!(reader.read(&mut buf).unwrap(), 8);

    let ctx = seen.read().expect("fn called");
    assert_eq!(ctx.current_pos, 100 + 8);
    assert_eq!(ctx.contiguous_read_start_pos, 100);
}

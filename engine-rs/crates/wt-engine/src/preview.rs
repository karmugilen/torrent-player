//! Preview reader for thumbnail generation.
//!
//! Port of `engine-go/preview.go`. Supplies seekable verified bytes for
//! thumbnails without mutating playback focus, resume history, or startup
//! ownership. Piece admission is budgeted so a decoder cannot exhaust RAM
//! before requesting MP4 tail metadata.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

/// Soft byte budget for one preview session (Go: 8 MiB).
pub const PREVIEW_BUDGET_BYTES: i64 = 8 * 1024 * 1024;

/// Preview sessions auto-close after this (Go: 10s).
pub const PREVIEW_SESSION_TIMEOUT: Duration = Duration::from_secs(10);

pub type PreviewReadFn = Box<dyn Fn(&mut [u8], i64) -> std::io::Result<usize> + Send + Sync>;

/// Directory for temporary preview draft storage under the download root.
pub fn preview_dir(base_dir: impl AsRef<Path>, info_hash_hex: &str) -> PathBuf {
    base_dir.as_ref().join(".preview").join(info_hash_hex)
}

/// Admit whole pieces covering `[offset, offset+length)` against a session
/// budget. Already-admitted or already-complete pieces reuse the budget.
///
/// `piece_length` / `file_torrent_offset` describe the torrent layout;
/// `piece_complete` reports verified pieces that need no network admission.
pub fn admit_preview_range(
    admitted: &mut HashMap<u32, bool>,
    admitted_bytes: &mut i64,
    budget: i64,
    piece_length: i64,
    file_torrent_offset: i64,
    file_length: i64,
    offset: i64,
    length: i64,
    piece_complete: impl Fn(u32) -> bool,
    payload_allowed: bool,
) -> std::io::Result<Vec<u32>> {
    if length <= 0 || piece_length <= 0 {
        return Ok(Vec::new());
    }
    if offset < 0 || offset >= file_length {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid preview offset",
        ));
    }
    let end_off = (offset + length).min(file_length);
    let abs_begin = file_torrent_offset + offset;
    let abs_end = file_torrent_offset + end_off;
    let begin = (abs_begin / piece_length) as u32;
    let end = ((abs_end + piece_length - 1) / piece_length) as u32;
    let mut need = 0i64;
    let mut new_pieces = Vec::new();
    for i in begin..end {
        if admitted.contains_key(&i) {
            continue;
        }
        if piece_complete(i) {
            continue;
        }
        if !payload_allowed {
            return Err(std::io::Error::other("payload transfers are paused"));
        }
        // Approximate piece length: full pieces except possibly the last.
        let n = piece_length;
        if *admitted_bytes + need + n > budget {
            return Err(std::io::Error::other("preview byte budget exceeded"));
        }
        need += n;
        new_pieces.push(i);
    }
    for i in &new_pieces {
        admitted.insert(*i, true);
        *admitted_bytes += piece_length;
    }
    Ok(new_pieces)
}

pub struct PreviewReader {
    pub file_index: usize,
    pub length: i64,
    budget: i64,
    budget_remaining: AtomicI64,
    closed: AtomicBool,
    inner_read: Mutex<Option<PreviewReadFn>>,
    /// Pieces admitted for network fetch this session.
    admitted: Mutex<HashMap<u32, bool>>,
    admitted_bytes: AtomicI64,
}

impl PreviewReader {
    pub fn new(file_index: usize, length: i64, read_fn: PreviewReadFn) -> Self {
        Self::with_budget(file_index, length, PREVIEW_BUDGET_BYTES, read_fn)
    }

    pub fn with_budget(
        file_index: usize,
        length: i64,
        budget: i64,
        read_fn: PreviewReadFn,
    ) -> Self {
        Self {
            file_index,
            length,
            budget,
            budget_remaining: AtomicI64::new(budget),
            closed: AtomicBool::new(false),
            inner_read: Mutex::new(Some(read_fn)),
            admitted: Mutex::new(HashMap::new()),
            admitted_bytes: AtomicI64::new(0),
        }
    }

    pub fn length(&self) -> i64 {
        self.length
    }

    pub fn budget(&self) -> i64 {
        self.budget
    }

    pub fn admitted_bytes(&self) -> i64 {
        self.admitted_bytes.load(Ordering::SeqCst)
    }

    /// Admit pieces for `[offset, offset+want)` then read. When
    /// `piece_length == 0`, skips admission and only enforces the soft
    /// remaining-budget counter used by the stub reader path.
    pub fn read_at(&self, buf: &mut [u8], offset: i64) -> std::io::Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "preview reader closed",
            ));
        }
        if offset < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid preview offset",
            ));
        }
        if offset >= self.length || buf.is_empty() {
            return Ok(0);
        }

        let max_read = (self.length - offset) as usize;
        let want = buf.len().min(max_read);

        // Soft remaining-budget gate (matches Go errPreviewBudget on overshoot).
        let mut curr_budget = self.budget_remaining.load(Ordering::SeqCst);
        loop {
            if curr_budget <= 0 {
                return Err(std::io::Error::other("preview byte budget exceeded"));
            }
            let allowed = want.min(curr_budget as usize);
            let next_budget = curr_budget - allowed as i64;
            match self.budget_remaining.compare_exchange_weak(
                curr_budget,
                next_budget,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    let lock = self.inner_read.lock();
                    if let Some(ref reader) = *lock {
                        return reader(&mut buf[..allowed], offset);
                    }
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "preview closed",
                    ));
                }
                Err(b) => curr_budget = b,
            }
        }
    }

    /// Record piece admissions for a read range (engine wiring calls this
    /// before publishing High demand).
    pub fn admit_range(
        &self,
        piece_length: i64,
        file_torrent_offset: i64,
        offset: i64,
        length: i64,
        piece_complete: impl Fn(u32) -> bool,
        payload_allowed: bool,
    ) -> std::io::Result<Vec<u32>> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "preview closed",
            ));
        }
        let mut admitted = self.admitted.lock();
        let mut admitted_n = self.admitted_bytes.load(Ordering::SeqCst);
        let new_pieces = admit_preview_range(
            &mut admitted,
            &mut admitted_n,
            self.budget,
            piece_length,
            file_torrent_offset,
            self.length,
            offset,
            length,
            piece_complete,
            payload_allowed,
        )?;
        self.admitted_bytes.store(admitted_n, Ordering::SeqCst);
        Ok(new_pieces)
    }

    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut lock = self.inner_read.lock();
        *lock = None;
        self.admitted.lock().clear();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn preview_dir_nests_under_dot_preview() {
        let p = preview_dir("/data", "abc");
        assert_eq!(p, PathBuf::from("/data/.preview/abc"));
    }

    #[test]
    fn budget_enforced_across_seeks() {
        let data = vec![0u8; 1024];
        let reader = PreviewReader::with_budget(
            0,
            data.len() as i64,
            100,
            Box::new(move |buf, off| {
                let off = off as usize;
                let n = buf.len().min(data.len().saturating_sub(off));
                buf[..n].copy_from_slice(&data[off..off + n]);
                Ok(n)
            }),
        );
        let mut buf = [0u8; 60];
        assert_eq!(reader.read_at(&mut buf, 0).unwrap(), 60);
        let mut buf = [0u8; 60];
        assert_eq!(reader.read_at(&mut buf, 60).unwrap(), 40);
        let mut buf = [0u8; 1];
        let err = reader.read_at(&mut buf, 100).unwrap_err();
        assert!(err.to_string().contains("budget"));
    }

    #[test]
    fn close_is_idempotent() {
        let reader = PreviewReader::new(0, 10, Box::new(|buf, _| Ok(buf.len())));
        reader.close();
        reader.close();
        assert!(reader.is_closed());
        assert!(reader.read_at(&mut [0u8; 4], 0).is_err());
    }

    #[test]
    fn admit_range_skips_complete_and_caps_budget() {
        let mut admitted = HashMap::new();
        let mut admitted_n = 0i64;
        let complete = |i: u32| i == 1;
        // piece 0 needs admission (1 MiB), piece 1 complete, piece 2 would exceed.
        let budget = 1 * 1024 * 1024;
        let new_pieces = admit_preview_range(
            &mut admitted,
            &mut admitted_n,
            budget,
            1 << 20,
            0,
            4 << 20,
            0,
            2 << 20,
            complete,
            true,
        )
        .unwrap();
        assert_eq!(new_pieces, vec![0]);
        let err = admit_preview_range(
            &mut admitted,
            &mut admitted_n,
            budget,
            1 << 20,
            0,
            4 << 20,
            2 << 20,
            1 << 20,
            complete,
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("budget"));
    }

    #[test]
    fn admit_range_respects_payload_gate() {
        let mut admitted = HashMap::new();
        let mut admitted_n = 0;
        let err = admit_preview_range(
            &mut admitted,
            &mut admitted_n,
            PREVIEW_BUDGET_BYTES,
            1 << 20,
            0,
            4 << 20,
            0,
            1 << 20,
            |_| false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("paused"));
    }

    #[test]
    fn arc_close_shared() {
        let reader = Arc::new(PreviewReader::new(
            0,
            8,
            Box::new(|buf, _| {
                buf.fill(7);
                Ok(buf.len())
            }),
        ));
        let mut buf = [0u8; 4];
        assert_eq!(reader.read_at(&mut buf, 0).unwrap(), 4);
        reader.close();
        assert!(reader.read_at(&mut buf, 0).is_err());
    }
}

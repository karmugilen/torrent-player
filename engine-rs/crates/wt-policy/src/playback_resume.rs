//! Resume-position hold (`engine-go/playback_resume.go`).
//!
//! Pure state machine: the engine supplies `now` and applies the returned
//! piece demand. Header/tail probes must not replace a useful body position
//! ([`ResumeRange::note_read`]).

use std::collections::HashMap;
use std::time::Instant;

use wt_types::{PieceIndex, PiecePriority};

use crate::constants::{PLAYBACK_RESUME_TTL, PLAYBACK_URGENT_BYTES, RESUME_WINDOW_MAX_BYTES};
use crate::piece_span::{file_piece_range, FileLayout};

/// One warm resume hold within the storage budget.
#[derive(Clone, Debug, Default)]
pub struct ResumeRange {
    pub file_index: Option<usize>,
    pub offset: i64,
    pub generation: u64,
    pub demand: HashMap<PieceIndex, PiecePriority>,
    pub expires_at: Option<Instant>,
    pub suspended: bool,
    /// Reader id that last wrote a useful body position (engine-assigned).
    pub resume_reader: Option<u64>,
}

/// Decision returned when a hold is armed or cleared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeHold {
    pub generation: u64,
    pub demand: HashMap<PieceIndex, PiecePriority>,
}

/// Inputs for [`ResumeRange::prepare`].
#[derive(Clone, Copy, Debug)]
pub struct ResumePrepareArgs<'a> {
    pub file_index: usize,
    pub file_length: i64,
    pub offset: Option<i64>,
    pub now: Instant,
    pub layout: &'a FileLayout,
    pub piece_length: i64,
    pub priority: PiecePriority,
}

impl ResumeRange {
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.demand.clear();
        self.expires_at = None;
    }

    pub fn position_for(&self, file_index: usize) -> i64 {
        if self.file_index == Some(file_index) {
            self.offset
        } else {
            0
        }
    }

    /// `/play` with an optional saved offset, or switch file → offset 0.
    pub fn prepare(&mut self, args: ResumePrepareArgs<'_>) -> ResumeHold {
        if let Some(off) = args.offset {
            self.resume_reader = None;
            self.file_index = Some(args.file_index);
            self.offset = off.max(0).min(args.file_length.saturating_sub(1).max(0));
        } else if self.file_index != Some(args.file_index) {
            self.file_index = Some(args.file_index);
            self.offset = 0;
            self.resume_reader = None;
        }
        self.hold(
            args.file_index,
            args.layout,
            args.piece_length,
            args.priority,
            args.now,
        )
    }

    /// Arm a hold around `offset` for `PLAYBACK_RESUME_TTL`.
    pub fn hold(
        &mut self,
        file_index: usize,
        layout: &FileLayout,
        piece_length: i64,
        priority: PiecePriority,
        now: Instant,
    ) -> ResumeHold {
        self.clear();
        if self.suspended || self.file_index != Some(file_index) || self.offset == 0 {
            return ResumeHold {
                generation: self.generation,
                demand: HashMap::new(),
            };
        }
        let end = (self.offset + PLAYBACK_URGENT_BYTES).min(layout.length);
        let span = file_piece_range(layout, piece_length, self.offset, end);
        for i in span.begin..span.end {
            self.demand.insert(i, priority);
        }
        self.expires_at = Some(now + PLAYBACK_RESUME_TTL);
        ResumeHold {
            generation: self.generation,
            demand: self.demand.clone(),
        }
    }

    /// Expire if `generation` still matches (stale timers are ignored).
    pub fn expire(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.clear();
        true
    }

    /// Tick-based expiry using injected `now` (no sleeping timers).
    pub fn tick_expire(&mut self, now: Instant) -> bool {
        if let Some(at) = self.expires_at {
            if now >= at {
                self.clear();
                return true;
            }
        }
        false
    }

    /// Record a useful body read; header/tail probes are ignored.
    pub fn note_read(
        &mut self,
        reader_id: u64,
        file_index: usize,
        offset: i64,
        file_length: i64,
        head_bytes: i64,
        tail_bytes: i64,
    ) {
        if offset < head_bytes || offset >= file_length - tail_bytes {
            return;
        }
        self.file_index = Some(file_index);
        self.offset = offset;
        self.resume_reader = Some(reader_id);
        if !self.demand.is_empty() {
            self.clear();
        }
    }

    /// On reader close: hold High if this reader owned the position and no
    /// sibling reader for the same file is still open.
    pub fn hold_on_close(
        &mut self,
        reader_id: u64,
        file_index: usize,
        other_readers_same_file: bool,
        layout: &FileLayout,
        piece_length: i64,
        now: Instant,
    ) -> ResumeHold {
        if other_readers_same_file {
            return ResumeHold {
                generation: self.generation,
                demand: self.demand.clone(),
            };
        }
        if self.resume_reader == Some(reader_id) {
            return self.hold(file_index, layout, piece_length, PiecePriority::High, now);
        }
        ResumeHold {
            generation: self.generation,
            demand: self.demand.clone(),
        }
    }

    /// Align and bound a verified persist window (engine copies bytes).
    pub fn aligned_persist_window(
        file: &FileLayout,
        piece_length: i64,
        offset: i64,
        piece_complete: impl Fn(PieceIndex) -> bool,
    ) -> Option<(i64, i64)> {
        if piece_length <= 0 {
            return None;
        }
        let mut abs = ((file.torrent_offset + offset.max(0)) / piece_length) * piece_length;
        if abs < file.torrent_offset {
            abs += piece_length;
        }
        let aligned = abs - file.torrent_offset;
        let end = (file.torrent_offset + file.length).min(abs + RESUME_WINDOW_MAX_BYTES);
        let mut length = 0i64;
        let mut at = abs;
        while at < end {
            let piece = (at / piece_length) as PieceIndex;
            let piece_len = piece_length.min(file.torrent_offset + file.length - at);
            if at + piece_len > end || !piece_complete(piece) {
                break;
            }
            length += piece_len;
            at += piece_len;
        }
        if length <= 0 {
            None
        } else {
            Some((aligned, length))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn layout(len: i64, piece_len: i64) -> FileLayout {
        let pieces = ((len + piece_len - 1) / piece_len) as PieceIndex;
        FileLayout {
            torrent_offset: 0,
            length: len,
            begin_piece: 0,
            end_piece: pieces,
        }
    }

    #[test]
    fn test_playback_close_reopen_preserves_resume_across_metadata_probes() {
        let mut r = ResumeRange::default();
        let piece_len = 1 << 20;
        let file_len = 24 * piece_len;
        let lay = layout(file_len, piece_len);
        let now = Instant::now();
        r.note_read(1, 0, 10 * piece_len, file_len, 5 << 20, 3 << 20);
        assert_eq!(r.position_for(0), 10 * piece_len);
        r.note_read(2, 0, 0, file_len, 5 << 20, 3 << 20);
        assert_eq!(r.position_for(0), 10 * piece_len);
        let hold = r.hold(0, &lay, piece_len, PiecePriority::High, now);
        assert!(!hold.demand.is_empty());
        let gen = hold.generation;
        assert!(!r.expire(gen.wrapping_sub(1)));
        assert!(!r.demand.is_empty());
        assert!(r.expire(gen));
        assert!(r.demand.is_empty());
    }

    #[test]
    fn tick_expire_uses_injected_time() {
        let mut r = ResumeRange::default();
        let piece_len = 1 << 20;
        let lay = layout(24 * piece_len, piece_len);
        let now = Instant::now();
        r.file_index = Some(0);
        r.offset = 10 * piece_len;
        r.hold(0, &lay, piece_len, PiecePriority::Now, now);
        assert!(!r.tick_expire(now + Duration::from_secs(1)));
        assert!(r.tick_expire(now + PLAYBACK_RESUME_TTL + Duration::from_millis(1)));
        assert!(r.demand.is_empty());
    }
}

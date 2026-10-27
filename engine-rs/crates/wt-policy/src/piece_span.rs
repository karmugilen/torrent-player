//! Half-open piece index spans and file→piece mapping helpers.
//!
//! The engine builds spans from torrent file layout and feeds them into
//! [`crate::playback_grid`], [`crate::video_priority`], and
//! [`crate::next_preload`].

use wt_types::PieceIndex;

/// Half-open piece index range `[begin, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PieceSpan {
    pub begin: PieceIndex,
    pub end: PieceIndex,
}

impl PieceSpan {
    pub fn new(begin: PieceIndex, end: PieceIndex) -> Self {
        Self { begin, end }
    }

    pub fn empty(self) -> bool {
        self.begin >= self.end
    }

    pub fn is_empty(self) -> bool {
        self.empty()
    }

    pub fn contains(self, i: PieceIndex) -> bool {
        i >= self.begin && i < self.end
    }

    pub fn overlaps(self, o: Self) -> bool {
        !self.empty() && !o.empty() && self.begin < o.end && o.begin < self.end
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.begin)
    }
}

/// Reader playhead window tiers (urgent tip, readahead, optional lookahead).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaybackWindow {
    pub urgent: PieceSpan,
    pub readahead: PieceSpan,
    pub lookahead: PieceSpan,
    pub open_phase: bool,
    pub head_boost: bool,
    pub tail_boost: bool,
}

/// Union of non-empty spans (bounding box). Empty inputs are skipped.
pub fn union_span(spans: &[PieceSpan]) -> PieceSpan {
    let mut out = PieceSpan::default();
    let mut first = true;
    for &s in spans {
        if s.empty() {
            continue;
        }
        if first {
            out = s;
            first = false;
            continue;
        }
        if s.begin < out.begin {
            out.begin = s.begin;
        }
        if s.end > out.end {
            out.end = s.end;
        }
    }
    out
}

/// Minimal file layout input for piece-range mapping (no wt-core dependency).
#[derive(Clone, Debug)]
pub struct FileLayout {
    /// Absolute byte offset of the file in the torrent payload.
    pub torrent_offset: i64,
    pub length: i64,
    pub begin_piece: PieceIndex,
    pub end_piece: PieceIndex,
}

/// Map a half-open byte range within a file to torrent piece indices `[begin, end)`.
pub fn file_piece_range(file: &FileLayout, piece_length: i64, start: i64, end: i64) -> PieceSpan {
    if piece_length <= 0 || start >= end {
        return PieceSpan::default();
    }
    let mut start = start;
    let mut end = end;
    if start < 0 {
        start = 0;
    }
    if end > file.length {
        end = file.length;
    }
    if start >= end {
        return PieceSpan::default();
    }
    let abs_start = file.torrent_offset + start;
    let abs_end = file.torrent_offset + end;
    let mut begin = (abs_start / piece_length) as PieceIndex;
    let mut end_piece = (((abs_end - 1) / piece_length) + 1) as PieceIndex;
    if begin < file.begin_piece {
        begin = file.begin_piece;
    }
    if end_piece > file.end_piece {
        end_piece = file.end_piece;
    }
    if begin > end_piece {
        return PieceSpan::new(end_piece, end_piece);
    }
    PieceSpan::new(begin, end_piece)
}

/// Convenience: `file_piece_range` as a span from file-relative bytes.
pub fn span_from_file(file: &FileLayout, piece_length: i64, start: i64, end: i64) -> PieceSpan {
    file_piece_range(file, piece_length, start, end)
}

/// Raise every piece in `span` to at least `prio` in `demand`.
pub fn raise_span(
    demand: &mut std::collections::HashMap<PieceIndex, wt_types::PiecePriority>,
    span: PieceSpan,
    prio: wt_types::PiecePriority,
) {
    for i in span.begin..span.end {
        match demand.get(&i) {
            Some(cur) if *cur >= prio => {}
            _ => {
                demand.insert(i, prio);
            }
        }
    }
}

//! Playback grid telemetry (`engine-go/playback_grid.go`) and HTTP range parse.
//!
//! The engine calls [`split_urgent`] when publishing piece priorities, and
//! [`playback_tiers`] / [`playback_detail`] when filling `/pieces` responses.

use wt_types::PieceIndex;

use crate::constants::{PLAYBACK_DETAIL_MAX, PLAYBACK_URGENT_BYTES, URGENT_NOW_EXTRA_PIECES};
use crate::piece_span::{union_span, PieceSpan, PlaybackWindow};

/// One labelled playback tier for the grid (`U` = this block, `N` = next).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlaybackPieceRange {
    pub start: PieceIndex,
    pub end: PieceIndex,
    pub priority: String,
}

/// Per-piece detail strip around the focus window.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlaybackDetail {
    pub start: PieceIndex,
    pub states: String,
    pub playhead: PieceIndex,
}

/// Split an urgent span into Now (playhead tip) and Next (rest of fast tier).
pub fn split_urgent(urgent: PieceSpan, piece_len: i64) -> (PieceSpan, PieceSpan) {
    let mut now_end = urgent.end;
    if piece_len > 0 {
        let extra = URGENT_NOW_EXTRA_PIECES.load(std::sync::atomic::Ordering::Relaxed);
        let tip = ((PLAYBACK_URGENT_BYTES + piece_len - 1) / piece_len) as i32 + extra;
        let tip_end = urgent.begin.saturating_add(tip.max(0) as u32);
        now_end = now_end.min(tip_end);
    }
    (
        PieceSpan::new(urgent.begin, now_end),
        PieceSpan::new(now_end, urgent.end),
    )
}

/// Build `U`/`N` tiers and playheads for one reader window.
pub fn playback_tiers(
    window: &PlaybackWindow,
    piece_len: i64,
) -> (Vec<PlaybackPieceRange>, Option<PieceIndex>) {
    let mut playback = Vec::new();
    let (now, _) = split_urgent(window.urgent, piece_len);
    let next_end = union_span(&[window.urgent, window.readahead, window.lookahead]).end;
    let next = PieceSpan::new(now.end, next_end);
    for (span, label) in [(now, "U"), (next, "N")] {
        if !span.empty() {
            playback.push(PlaybackPieceRange {
                start: span.begin,
                end: span.end - 1,
                priority: label.into(),
            });
        }
    }
    let playhead = if !window.urgent.empty() {
        Some(window.urgent.begin)
    } else {
        None
    };
    (playback, playhead)
}

/// Focus window: playing reader's window, or the widest open window.
pub fn playback_focus_window<'a>(
    playing: Option<&'a PlaybackWindow>,
    windows: &'a [PlaybackWindow],
) -> Option<&'a PlaybackWindow> {
    if let Some(w) = playing {
        return Some(w);
    }
    let mut best: Option<&PlaybackWindow> = None;
    let mut width = -1i32;
    for w in windows {
        let span = union_span(&[w.urgent, w.readahead, w.lookahead]);
        let wlen = span.end as i32 - span.begin as i32;
        if wlen > width {
            best = Some(w);
            width = wlen;
        }
    }
    best
}

/// Build the per-piece detail strip. `state_at(i)` returns `v`/`s`/`r`/`m`.
pub fn playback_detail(
    window: &PlaybackWindow,
    num_pieces: PieceIndex,
    mut state_at: impl FnMut(PieceIndex) -> u8,
) -> Option<PlaybackDetail> {
    if window.urgent.empty() {
        return None;
    }
    let mut span = union_span(&[window.urgent, window.readahead, window.lookahead]);
    let cap = span
        .begin
        .saturating_add(PLAYBACK_DETAIL_MAX as u32)
        .min(num_pieces);
    span.end = span.end.min(cap);
    if span.empty() {
        return None;
    }
    let mut states = Vec::with_capacity((span.end - span.begin) as usize);
    for i in span.begin..span.end {
        states.push(state_at(i));
    }
    Some(PlaybackDetail {
        start: span.begin,
        states: String::from_utf8(states).unwrap_or_default(),
        playhead: window.urgent.begin,
    })
}

/// Parse a single HTTP `Range` header start offset. Returns -1 when invalid.
pub fn range_start(header: &str) -> i64 {
    let trimmed = header.trim();
    let Some(spec) = trimmed.strip_prefix("bytes=") else {
        return -1;
    };
    if spec.contains(',') {
        return -1;
    }
    let Some((first, _)) = spec.split_once('-') else {
        return -1;
    };
    let first = first.trim();
    if first.is_empty() {
        return -1;
    }
    match first.parse::<i64>() {
        Ok(n) if n >= 0 => n,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_start_parses_single_ranges() {
        let cases = [
            ("bytes=0-", 0),
            ("bytes=1048576-", 1 << 20),
            ("bytes=10-20", 10),
            (" bytes=7- ", 7),
            ("", -1),
            ("bytes=-500", -1),
            ("bytes=1-2,5-6", -1),
            ("items=1-", -1),
            ("bytes=x-", -1),
        ];
        for (h, want) in cases {
            assert_eq!(range_start(h), want, "{h}");
        }
    }

    #[test]
    fn split_urgent_caps_now_at_two_mib() {
        let piece_len = 1 << 20;
        let urgent = PieceSpan::new(40, 100);
        let (now, next) = split_urgent(urgent, piece_len);
        // 2 MiB / 1 MiB = 2 pieces (+0 extra)
        assert_eq!(now, PieceSpan::new(40, 42));
        assert_eq!(next, PieceSpan::new(42, 100));
    }

    #[test]
    fn tiers_label_u_then_n() {
        let w = PlaybackWindow {
            urgent: PieceSpan::new(40, 45),
            readahead: PieceSpan::new(45, 60),
            lookahead: PieceSpan::default(),
            ..Default::default()
        };
        let (tiers, playhead) = playback_tiers(&w, 1 << 20);
        assert_eq!(playhead, Some(40));
        assert_eq!(tiers[0].priority, "U");
        assert_eq!(tiers[0].start, 40);
        assert!(tiers[0].end - tiers[0].start + 1 <= 3);
        assert_eq!(tiers[1].priority, "N");
        assert_eq!(tiers[1].start, tiers[0].end + 1);
    }
}

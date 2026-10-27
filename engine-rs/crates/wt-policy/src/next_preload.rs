//! Play-next episode warm-up (`engine-go/next_preload.go`).
//!
//! On `/play` the engine calls [`NextPreload::set_plan`]. After each window
//! update the reader asks [`NextPreload::should_start`]; when true, call
//! [`NextPreload::activate`] and merge [`NextPreload::pieces`] into startup
//! hints at High.

use std::collections::{HashMap, HashSet};

use wt_types::PieceIndex;

use crate::constants::{
    DIRECT_STARTUP_HEAD, DIRECT_STARTUP_TAIL, NEXT_PRELOAD_AT, VIDEO_STARTUP_HEAD_BYTES,
    VIDEO_STARTUP_TAIL_BYTES,
};
use crate::piece_span::{span_from_file, FileLayout};
use crate::video_priority::is_video_file_name;

/// Status API view of the warm-up plan.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NextPreloadView {
    #[serde(rename = "fileIndex")]
    pub file_index: usize,
    pub active: bool,
    pub ready: bool,
}

/// Next-episode preload plan for one torrent record.
#[derive(Clone, Debug, Default)]
pub struct NextPreload {
    current: Option<usize>,
    next: Option<usize>,
    active: bool,
    pieces: HashSet<PieceIndex>,
}

/// File descriptor needed to validate / plan a preload target.
#[derive(Clone, Debug)]
pub struct PreloadFile {
    pub index: usize,
    pub display_path: String,
    pub layout: FileLayout,
    pub selected: bool,
}

/// Reader/record flags for the preload start gate.
#[derive(Clone, Copy, Debug)]
pub struct PreloadStartGate {
    pub current: usize,
    pub offset: i64,
    pub file_length: i64,
    pub open_phase: bool,
    pub scrubbing: bool,
    pub already_asked: bool,
    pub paused: bool,
    pub configured: bool,
    pub removed: bool,
    pub has_error: bool,
}

impl NextPreload {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Replace the plan for a new `/play`. Same-file next is ignored.
    pub fn set_plan(&mut self, current: usize, next: Option<usize>, files: &[PreloadFile]) {
        self.clear();
        let Some(n) = next else {
            return;
        };
        if n == current {
            return;
        }
        let Some(f) = files.iter().find(|f| f.index == n) else {
            return;
        };
        if !f.selected || !is_video_file_name(&f.display_path) {
            return;
        }
        self.current = Some(current);
        self.next = Some(n);
    }

    pub fn planned_next(&self) -> Option<usize> {
        self.next
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn pieces(&self) -> &HashSet<PieceIndex> {
        &self.pieces
    }

    /// True once when playback of `current` reaches [`NEXT_PRELOAD_AT`].
    pub fn should_start(&self, gate: PreloadStartGate) -> bool {
        if gate.already_asked
            || gate.open_phase
            || gate.scrubbing
            || gate.paused
            || !gate.configured
            || gate.removed
            || gate.has_error
        {
            return false;
        }
        if self.active || self.current != Some(gate.current) || self.next.is_none() {
            return false;
        }
        if gate.file_length <= 0 {
            return false;
        }
        (gate.offset as f64) >= (gate.file_length as f64) * NEXT_PRELOAD_AT
    }

    /// Mark active and compute head/tail pieces for the next episode.
    ///
    /// When Direct Play, `override_pieces` may supply the durable startup set.
    pub fn activate(
        &mut self,
        next_file: &PreloadFile,
        piece_length: i64,
        direct_play: bool,
        override_pieces: Option<HashSet<PieceIndex>>,
    ) {
        if self.active {
            return;
        }
        self.active = true;
        if let Some(p) = override_pieces {
            self.pieces = p;
            return;
        }
        let (head, tail) = if direct_play {
            (DIRECT_STARTUP_HEAD, DIRECT_STARTUP_TAIL)
        } else {
            (VIDEO_STARTUP_HEAD_BYTES, VIDEO_STARTUP_TAIL_BYTES)
        };
        let length = next_file.layout.length;
        let mut pieces = HashSet::new();
        let head_span = span_from_file(&next_file.layout, piece_length, 0, length.min(head));
        for p in head_span.begin..head_span.end {
            pieces.insert(p);
        }
        if length > tail {
            let tail_span = span_from_file(&next_file.layout, piece_length, length - tail, length);
            for p in tail_span.begin..tail_span.end {
                pieces.insert(p);
            }
        }
        self.pieces = pieces;
    }

    pub fn view(&self, piece_complete: impl Fn(PieceIndex) -> bool) -> Option<NextPreloadView> {
        let next = self.next?;
        let mut view = NextPreloadView {
            file_index: next,
            active: self.active,
            ready: false,
        };
        if self.active && !self.pieces.is_empty() {
            view.ready = self.pieces.iter().all(|&p| piece_complete(p));
        }
        Some(view)
    }

    /// Raise active preload pieces into a demand map at High.
    pub fn raise_into(&self, demand: &mut HashMap<PieceIndex, wt_types::PiecePriority>) {
        if !self.active {
            return;
        }
        for &p in &self.pieces {
            demand
                .entry(p)
                .and_modify(|cur| {
                    if *cur < wt_types::PiecePriority::High {
                        *cur = wt_types::PiecePriority::High;
                    }
                })
                .or_insert(wt_types::PiecePriority::High);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(index: usize, selected: bool) -> PreloadFile {
        PreloadFile {
            index,
            display_path: format!("ep{index}.mp4"),
            layout: FileLayout {
                torrent_offset: index as i64 * (8 << 20),
                length: 8 << 20,
                begin_piece: (index as u32) * 8,
                end_piece: (index as u32) * 8 + 8,
            },
            selected,
        }
    }

    #[test]
    fn test_direct_play_preloads_next_episode_near_the_end_plan_gate() {
        let files = [video(0, true), video(1, true)];
        let mut np = NextPreload::default();
        np.set_plan(0, None, &files);
        assert!(np.planned_next().is_none());
        np.set_plan(0, Some(0), &files);
        assert!(np.planned_next().is_none());
        np.set_plan(0, Some(1), &files);
        assert_eq!(np.planned_next(), Some(1));
        assert!(!np.is_active());
    }

    fn gate(offset: i64, len: i64, open_phase: bool) -> PreloadStartGate {
        PreloadStartGate {
            current: 0,
            offset,
            file_length: len,
            open_phase,
            scrubbing: false,
            already_asked: false,
            paused: false,
            configured: true,
            removed: false,
            has_error: false,
        }
    }

    #[test]
    fn test_direct_play_preloads_next_episode_near_the_end_start_gate() {
        let files = [video(0, true), video(1, true)];
        let mut np = NextPreload::default();
        np.set_plan(0, Some(1), &files);
        let len = 8 << 20;
        assert!(!np.should_start(gate(0, len, false)));
        assert!(!np.should_start(gate(3 << 20, len, false)));
        assert!(!np.should_start(gate((len as f64 * 0.95) as i64, len, true))); // open phase
        assert!(np.should_start(gate((len as f64 * 0.95) as i64, len, false)));
        np.activate(&files[1], 1 << 20, true, None);
        assert!(np.is_active());
        assert!(!np.pieces().is_empty());
        let view = np.view(|_| false).unwrap();
        assert!(view.active && !view.ready);
        let view = np.view(|_| true).unwrap();
        assert!(view.ready);
    }
}

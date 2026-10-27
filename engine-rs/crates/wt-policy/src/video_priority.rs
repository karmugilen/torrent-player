//! Video startup head/tail hints (`engine-go/video_priority.go`).
//!
//! Call [`video_startup_hints`] after selection/focus changes. Merge the
//! returned demand into the playback coordinator's hint map (engine side).
//! Mid-file pieces stay at Normal so they download concurrently.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use wt_types::{PieceIndex, PiecePriority};

use crate::constants::{
    DIRECT_STARTUP_HEAD, DIRECT_STARTUP_TAIL, PLAYBACK_BOOST_HEAD_BYTES, PLAYBACK_BOOST_TAIL_BYTES,
    VIDEO_STARTUP_HEAD_BYTES, VIDEO_STARTUP_TAIL_BYTES,
};
use crate::piece_span::{raise_span, span_from_file, FileLayout};

/// One selected/candidate file for startup hint calculation.
#[derive(Clone, Debug)]
pub struct VideoFileInput {
    pub index: usize,
    pub display_path: String,
    pub layout: FileLayout,
}

/// Engine-visible record flags that gate startup hints.
#[derive(Clone, Debug, Default)]
pub struct VideoHintContext {
    pub transfers_allowed: bool,
    pub paused: bool,
    pub configured: bool,
    pub checking: bool,
    pub removed: bool,
    pub has_error: bool,
    pub direct_play: bool,
    pub file_only: bool,
    pub focused_file: Option<usize>,
    pub selected: HashSet<usize>,
    pub piece_length: i64,
    pub ghost_pro: bool,
    /// Extra pieces already planned by next-preload (raised to High if below).
    pub next_preload_pieces: Vec<PieceIndex>,
    /// Direct-play startup cache pieces at Normal (below playback demand).
    pub startup_fetch_pieces: Vec<PieceIndex>,
}

/// True when `name` looks like a video container (mime + torrent-common extras).
pub fn is_video_file_name(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext.is_empty() {
        return false;
    }
    if mime_is_video(&ext) {
        return true;
    }
    matches!(
        ext.as_str(),
        "mkv" | "ts" | "m2ts" | "mts" | "vob" | "m4v" | "f4v" | "mpg" | "mpeg" | "m2v"
    )
}

fn mime_is_video(ext: &str) -> bool {
    // Subset of Go `mime.TypeByExtension` video/* results we care about.
    matches!(
        ext,
        "mp4"
            | "m4v"
            | "webm"
            | "ogv"
            | "mov"
            | "avi"
            | "3gp"
            | "3g2"
            | "mpeg"
            | "mpg"
            | "mpe"
            | "mpv"
            | "m2v"
    )
}

/// Compute soft Now hints for selected video head/tail pieces.
///
/// Returns an empty map when transfers are gated off (paused, checking, …).
pub fn video_startup_hints(
    files: &[VideoFileInput],
    ctx: &VideoHintContext,
) -> HashMap<PieceIndex, PiecePriority> {
    let mut demand = HashMap::new();
    if !ctx.transfers_allowed
        || ctx.paused
        || !ctx.configured
        || ctx.checking
        || ctx.removed
        || ctx.has_error
        || ctx.piece_length <= 0
    {
        return demand;
    }
    for f in files {
        if !ctx.selected.contains(&f.index) || !is_video_file_name(&f.display_path) {
            continue;
        }
        if (ctx.direct_play || ctx.file_only)
            && ctx.focused_file.map(|i| i != f.index).unwrap_or(true)
        {
            continue;
        }
        let length = f.layout.length;
        if length <= 0 {
            continue;
        }
        let (mut head, mut tail) = if ctx.direct_play {
            (DIRECT_STARTUP_HEAD, DIRECT_STARTUP_TAIL)
        } else {
            (VIDEO_STARTUP_HEAD_BYTES, VIDEO_STARTUP_TAIL_BYTES)
        };
        if ctx.ghost_pro && !ctx.direct_play {
            let piece_len = ctx.piece_length;
            head = head.max(PLAYBACK_BOOST_HEAD_BYTES).max(2 * piece_len);
            tail = tail.max(PLAYBACK_BOOST_TAIL_BYTES).max(2 * piece_len);
        }
        let head_span = span_from_file(&f.layout, ctx.piece_length, 0, length.min(head));
        raise_span(&mut demand, head_span, PiecePriority::Now);
        if length > tail {
            let tail_span = span_from_file(&f.layout, ctx.piece_length, length - tail, length);
            raise_span(&mut demand, tail_span, PiecePriority::Now);
        }
    }
    for p in &ctx.startup_fetch_pieces {
        demand
            .entry(*p)
            .and_modify(|cur| {
                if *cur < PiecePriority::Normal {
                    *cur = PiecePriority::Normal;
                }
            })
            .or_insert(PiecePriority::Normal);
    }
    for p in &ctx.next_preload_pieces {
        demand
            .entry(*p)
            .and_modify(|cur| {
                if *cur < PiecePriority::High {
                    *cur = PiecePriority::High;
                }
            })
            .or_insert(PiecePriority::High);
    }
    demand
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_video_file_name_cases() {
        let cases = [
            ("clip.mp4", true),
            ("Show.MKV", true),
            ("a/b/c.webm", true),
            ("notes.txt", false),
            ("archive.zip", false),
            ("track.flac", false),
            ("movie", false),
            ("sample.m4v", true),
        ];
        for (name, want) in cases {
            assert_eq!(is_video_file_name(name), want, "{name}");
        }
    }

    fn layout(pieces: u32, piece_len: i64) -> FileLayout {
        FileLayout {
            torrent_offset: 0,
            length: pieces as i64 * piece_len,
            begin_piece: 0,
            end_piece: pieces,
        }
    }

    #[test]
    fn startup_hints_raise_head_and_tail() {
        let piece_len = 1024 * 1024i64;
        let pieces = 48u32;
        let files = [VideoFileInput {
            index: 0,
            display_path: "movie.mp4".into(),
            layout: layout(pieces, piece_len),
        }];
        let mut selected = HashSet::new();
        selected.insert(0);
        let ctx = VideoHintContext {
            transfers_allowed: true,
            configured: true,
            selected,
            piece_length: piece_len,
            ..Default::default()
        };
        let demand = video_startup_hints(&files, &ctx);
        let head_end = (VIDEO_STARTUP_HEAD_BYTES / piece_len) as PieceIndex;
        let tail_begin = pieces - (VIDEO_STARTUP_TAIL_BYTES / piece_len) as PieceIndex;
        assert_eq!(demand.get(&0), Some(&PiecePriority::Now));
        assert_eq!(demand.get(&(head_end - 1)), Some(&PiecePriority::Now));
        assert_eq!(demand.get(&tail_begin), Some(&PiecePriority::Now));
        assert_eq!(demand.get(&(pieces - 1)), Some(&PiecePriority::Now));
        let middle = head_end;
        assert_ne!(demand.get(&middle), Some(&PiecePriority::Now));
    }

    #[test]
    fn startup_hints_skip_non_video_and_paused() {
        let piece_len = 1024 * 1024i64;
        let files = [
            VideoFileInput {
                index: 0,
                display_path: "clip.mp4".into(),
                layout: FileLayout {
                    torrent_offset: 0,
                    length: 20 * piece_len,
                    begin_piece: 0,
                    end_piece: 20,
                },
            },
            VideoFileInput {
                index: 1,
                display_path: "notes.txt".into(),
                layout: FileLayout {
                    torrent_offset: 20 * piece_len,
                    length: 4 * piece_len,
                    begin_piece: 20,
                    end_piece: 24,
                },
            },
        ];
        let mut selected = HashSet::new();
        selected.insert(0);
        selected.insert(1);
        let mut ctx = VideoHintContext {
            transfers_allowed: true,
            configured: true,
            selected,
            piece_length: piece_len,
            ..Default::default()
        };
        let demand = video_startup_hints(&files, &ctx);
        assert_eq!(demand.get(&0), Some(&PiecePriority::Now));
        assert!(!demand.contains_key(&22));

        ctx.paused = true;
        assert!(video_startup_hints(&files, &ctx).is_empty());
    }
}

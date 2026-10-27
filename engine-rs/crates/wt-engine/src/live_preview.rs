//! Live preview state snapshots for thumbnail UIs.
//!
//! Port of `engine-go/live_preview.go`. A small read-only snapshot lets
//! thumbnails follow cached playback data without opening a preview reader.

use crate::types::PreviewStateResponse;

/// Inputs for building a `/preview-state` response.
#[derive(Clone, Debug, Default)]
pub struct PreviewStateInput {
    /// True when a playback reader is open on the requested file.
    pub active: bool,
    /// Resume / playhead byte offset within the file when known.
    pub byte_offset: i64,
    /// Absolute torrent offsets of completed pieces that overlap the file.
    /// Used to build the cache signature.
    pub completed_piece_indices: Vec<u32>,
}

/// XOR-fold completed piece indices the way Go does
/// (`piece+1) * 0x9e3779b97f4a7c15`), returned as lowercase hex.
pub fn preview_cache_signature(completed_piece_indices: &[u32]) -> String {
    let mut signature: u64 = 0;
    for &piece in completed_piece_indices {
        signature ^= (u64::from(piece) + 1).wrapping_mul(0x9e3779b97f4a7c15);
    }
    format!("{signature:x}")
}

/// Pieces whose byte span overlaps `[file_offset, file_offset+file_length)`.
pub fn pieces_overlapping_file(
    completed: impl IntoIterator<Item = (u32, bool)>,
    piece_length: i64,
    file_offset: i64,
    file_length: i64,
) -> Vec<u32> {
    if piece_length <= 0 || file_length <= 0 {
        return Vec::new();
    }
    let file_end = file_offset + file_length;
    let mut out = Vec::new();
    for (piece, complete) in completed {
        if !complete {
            continue;
        }
        let at = i64::from(piece) * piece_length;
        // Piece length is treated as piece_length for overlap; the Go code uses
        // the real last-piece length, but overlap with a full span is a
        // conservative superset for signature purposes.
        let piece_end = at + piece_length;
        if at < file_end && piece_end > file_offset {
            out.push(piece);
        }
    }
    out.sort_unstable();
    out
}

pub fn build_preview_state(input: PreviewStateInput) -> PreviewStateResponse {
    PreviewStateResponse {
        active: input.active,
        byte_offset: input.byte_offset,
        signature: preview_cache_signature(&input.completed_piece_indices),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_stable_for_same_pieces() {
        let a = preview_cache_signature(&[0, 2, 5]);
        let b = preview_cache_signature(&[0, 2, 5]);
        assert_eq!(a, b);
        assert_ne!(preview_cache_signature(&[0]), preview_cache_signature(&[1]));
    }

    #[test]
    fn overlapping_pieces_respect_file_window() {
        // piece_length = 100; file at [150, 350) → pieces 1 and 2.
        let completed = [(0u32, true), (1, true), (2, true), (3, true), (4, false)];
        let pieces = pieces_overlapping_file(completed, 100, 150, 200);
        assert_eq!(pieces, vec![1, 2, 3]);
    }

    #[test]
    fn build_preview_state_json_shape() {
        let resp = build_preview_state(PreviewStateInput {
            active: true,
            byte_offset: 42,
            completed_piece_indices: vec![1],
        });
        assert!(resp.active);
        assert_eq!(resp.byte_offset, 42);
        assert_eq!(resp.signature, preview_cache_signature(&[1]));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["active"], true);
        assert_eq!(v["byteOffset"], 42);
        assert!(v["signature"].is_string());
    }
}

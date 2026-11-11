//! Compact little-endian piece telemetry for `GET /pieces-bin/{id}`.
//!
//! Layout (version 1):
//! ```text
//! magic[4] = b"WTPC"
//! version: u16 LE = 1
//! flags: u16 LE
//!   bit0 = ghost_pro_mode
//! generation: i64 LE
//! timestamp: i64 LE
//! total_pieces: u32 LE
//! max_buckets: u32 LE
//! piece_length: i64 LE
//! last_piece_length: i64 LE
//! id_len: u16 LE
//! info_hash_len: u16 LE
//! bucket_count: u16 LE
//! playback_count: u16 LE
//! playhead_count: u16 LE
//! detail_states_len: u16 LE
//! detail_start: u32 LE
//! detail_playhead: u32 LE
//! reserved: u32 LE (= 0)
//! id[id_len] UTF-8
//! info_hash[info_hash_len] UTF-8 hex (empty when absent)
//! buckets[bucket_count]: each 8×u32 LE
//!   start, end, total, selected, verified, receiving,
//!   selected_verified, selected_receiving
//! playback[playback_count]: start u32, end u32, priority u8, pad[3]
//! playheads[playhead_count]: u32 LE each
//! detail_states[detail_states_len] UTF-8 (empty when absent)
//! ```

use crate::types::{
    PieceBucket, PieceTelemetryResponse, PlaybackDetail, PlaybackPieceRange,
};

pub const PIECES_BIN_MAGIC: &[u8; 4] = b"WTPC";
pub const PIECES_BIN_VERSION: u16 = 1;
const FLAG_GHOST_PRO: u16 = 1;
const HEADER_SIZE: usize = 72;
const BUCKET_SIZE: usize = 32;
const PLAYBACK_SIZE: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiecesBinError {
    Truncated,
    BadMagic,
    BadVersion(u16),
    BadUtf8(&'static str),
    BadPriority,
    LengthMismatch,
}

impl std::fmt::Display for PiecesBinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated pieces-bin payload"),
            Self::BadMagic => write!(f, "bad pieces-bin magic"),
            Self::BadVersion(v) => write!(f, "unsupported pieces-bin version {v}"),
            Self::BadUtf8(field) => write!(f, "invalid utf-8 in {field}"),
            Self::BadPriority => write!(f, "invalid playback priority byte"),
            Self::LengthMismatch => write!(f, "pieces-bin length mismatch"),
        }
    }
}

impl std::error::Error for PiecesBinError {}

pub fn encode_pieces_bin(telemetry: &PieceTelemetryResponse) -> Vec<u8> {
    let id = telemetry.id.as_bytes();
    let info_hash = telemetry
        .info_hash
        .as_deref()
        .unwrap_or("")
        .as_bytes();
    let playback = telemetry.playback.as_deref().unwrap_or(&[]);
    let playheads = telemetry.playheads.as_deref().unwrap_or(&[]);
    let detail = telemetry.playback_detail.as_ref();
    let detail_states = detail.map(|d| d.states.as_bytes()).unwrap_or(&[]);
    let detail_start = detail.map(|d| d.start as u32).unwrap_or(0);
    let detail_playhead = detail.map(|d| d.playhead as u32).unwrap_or(0);

    let mut flags = 0u16;
    if telemetry.ghost_pro_mode {
        flags |= FLAG_GHOST_PRO;
    }

    let capacity = HEADER_SIZE
        + id.len()
        + info_hash.len()
        + telemetry.buckets.len() * BUCKET_SIZE
        + playback.len() * PLAYBACK_SIZE
        + playheads.len() * 4
        + detail_states.len();
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(PIECES_BIN_MAGIC);
    out.extend_from_slice(&PIECES_BIN_VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&telemetry.generation.to_le_bytes());
    out.extend_from_slice(&telemetry.timestamp.to_le_bytes());
    out.extend_from_slice(&(telemetry.total_pieces as u32).to_le_bytes());
    out.extend_from_slice(&(telemetry.max_buckets as u32).to_le_bytes());
    out.extend_from_slice(&telemetry.piece_length.to_le_bytes());
    out.extend_from_slice(&telemetry.last_piece_length.to_le_bytes());
    out.extend_from_slice(&(id.len() as u16).to_le_bytes());
    out.extend_from_slice(&(info_hash.len() as u16).to_le_bytes());
    out.extend_from_slice(&(telemetry.buckets.len() as u16).to_le_bytes());
    out.extend_from_slice(&(playback.len() as u16).to_le_bytes());
    out.extend_from_slice(&(playheads.len() as u16).to_le_bytes());
    out.extend_from_slice(&(detail_states.len() as u16).to_le_bytes());
    out.extend_from_slice(&detail_start.to_le_bytes());
    out.extend_from_slice(&detail_playhead.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(id);
    out.extend_from_slice(info_hash);
    for b in &telemetry.buckets {
        out.extend_from_slice(&(b.start as u32).to_le_bytes());
        out.extend_from_slice(&(b.end as u32).to_le_bytes());
        out.extend_from_slice(&(b.total as u32).to_le_bytes());
        out.extend_from_slice(&(b.selected as u32).to_le_bytes());
        out.extend_from_slice(&(b.verified as u32).to_le_bytes());
        out.extend_from_slice(&(b.receiving as u32).to_le_bytes());
        out.extend_from_slice(&(b.selected_verified as u32).to_le_bytes());
        out.extend_from_slice(&(b.selected_receiving as u32).to_le_bytes());
    }
    for range in playback {
        out.extend_from_slice(&(range.start as u32).to_le_bytes());
        out.extend_from_slice(&(range.end as u32).to_le_bytes());
        let priority = match range.priority.as_str() {
            "U" => b'U',
            "N" => b'N',
            "R" => b'R',
            "L" => b'L',
            "B" => b'B',
            other => other.as_bytes().first().copied().unwrap_or(b'?'),
        };
        out.push(priority);
        out.extend_from_slice(&[0, 0, 0]);
    }
    for head in playheads {
        out.extend_from_slice(&(*head as u32).to_le_bytes());
    }
    out.extend_from_slice(detail_states);
    out
}

pub fn decode_pieces_bin(bytes: &[u8]) -> Result<PieceTelemetryResponse, PiecesBinError> {
    if bytes.len() < HEADER_SIZE {
        return Err(PiecesBinError::Truncated);
    }
    if &bytes[0..4] != PIECES_BIN_MAGIC {
        return Err(PiecesBinError::BadMagic);
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != PIECES_BIN_VERSION {
        return Err(PiecesBinError::BadVersion(version));
    }
    let flags = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    let generation = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let timestamp = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let total_pieces = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
    let max_buckets = u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as usize;
    let piece_length = i64::from_le_bytes(bytes[32..40].try_into().unwrap());
    let last_piece_length = i64::from_le_bytes(bytes[40..48].try_into().unwrap());
    let id_len = u16::from_le_bytes(bytes[48..50].try_into().unwrap()) as usize;
    let info_hash_len = u16::from_le_bytes(bytes[50..52].try_into().unwrap()) as usize;
    let bucket_count = u16::from_le_bytes(bytes[52..54].try_into().unwrap()) as usize;
    let playback_count = u16::from_le_bytes(bytes[54..56].try_into().unwrap()) as usize;
    let playhead_count = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    let detail_states_len = u16::from_le_bytes(bytes[58..60].try_into().unwrap()) as usize;
    let detail_start = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
    let detail_playhead = u32::from_le_bytes(bytes[64..68].try_into().unwrap()) as usize;

    let mut off = HEADER_SIZE;
    let need = id_len
        + info_hash_len
        + bucket_count * BUCKET_SIZE
        + playback_count * PLAYBACK_SIZE
        + playhead_count * 4
        + detail_states_len;
    if bytes.len() < off + need {
        return Err(PiecesBinError::Truncated);
    }
    if bytes.len() != off + need {
        return Err(PiecesBinError::LengthMismatch);
    }

    let id = std::str::from_utf8(&bytes[off..off + id_len])
        .map_err(|_| PiecesBinError::BadUtf8("id"))?
        .to_string();
    off += id_len;
    let info_hash = if info_hash_len == 0 {
        None
    } else {
        Some(
            std::str::from_utf8(&bytes[off..off + info_hash_len])
                .map_err(|_| PiecesBinError::BadUtf8("infoHash"))?
                .to_string(),
        )
    };
    off += info_hash_len;

    let mut buckets = Vec::with_capacity(bucket_count);
    for _ in 0..bucket_count {
        let start = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let end = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap()) as usize;
        let total = u32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap()) as usize;
        let selected = u32::from_le_bytes(bytes[off + 12..off + 16].try_into().unwrap()) as usize;
        let verified = u32::from_le_bytes(bytes[off + 16..off + 20].try_into().unwrap()) as usize;
        let receiving = u32::from_le_bytes(bytes[off + 20..off + 24].try_into().unwrap()) as usize;
        let selected_verified =
            u32::from_le_bytes(bytes[off + 24..off + 28].try_into().unwrap()) as usize;
        let selected_receiving =
            u32::from_le_bytes(bytes[off + 28..off + 32].try_into().unwrap()) as usize;
        buckets.push(PieceBucket {
            start,
            end,
            total,
            selected,
            verified,
            receiving,
            selected_verified,
            selected_receiving,
        });
        off += BUCKET_SIZE;
    }

    let mut playback = Vec::with_capacity(playback_count);
    for _ in 0..playback_count {
        let start = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let end = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap()) as usize;
        let priority = match bytes[off + 8] {
            b'U' => "U",
            b'N' => "N",
            b'R' => "R",
            b'L' => "L",
            b'B' => "B",
            _ => return Err(PiecesBinError::BadPriority),
        }
        .to_string();
        playback.push(PlaybackPieceRange {
            start,
            end,
            priority,
        });
        off += PLAYBACK_SIZE;
    }

    let mut playheads = Vec::with_capacity(playhead_count);
    for _ in 0..playhead_count {
        playheads.push(u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize);
        off += 4;
    }

    let playback_detail = if detail_states_len == 0 {
        None
    } else {
        let states = std::str::from_utf8(&bytes[off..off + detail_states_len])
            .map_err(|_| PiecesBinError::BadUtf8("playbackDetail.states"))?
            .to_string();
        Some(PlaybackDetail {
            start: detail_start,
            states,
            playhead: detail_playhead,
        })
    };

    Ok(PieceTelemetryResponse {
        id,
        info_hash,
        generation,
        timestamp,
        total_pieces,
        piece_length,
        last_piece_length,
        max_buckets,
        buckets,
        ghost_pro_mode: flags & FLAG_GHOST_PRO != 0,
        playback: (!playback.is_empty()).then_some(playback),
        playheads: (!playheads.is_empty()).then_some(playheads),
        playback_detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PieceTelemetryResponse {
        PieceTelemetryResponse {
            id: "abc".into(),
            info_hash: Some("deadbeef".into()),
            generation: 3,
            timestamp: 1_700_000_000_000,
            total_pieces: 4,
            piece_length: 16_384,
            last_piece_length: 1024,
            max_buckets: 256,
            buckets: vec![
                PieceBucket {
                    start: 0,
                    end: 1,
                    total: 2,
                    selected: 2,
                    verified: 1,
                    receiving: 1,
                    selected_verified: 1,
                    selected_receiving: 1,
                },
                PieceBucket {
                    start: 2,
                    end: 3,
                    total: 2,
                    selected: 1,
                    verified: 2,
                    receiving: 0,
                    selected_verified: 1,
                    selected_receiving: 0,
                },
            ],
            ghost_pro_mode: true,
            playback: Some(vec![
                PlaybackPieceRange {
                    start: 0,
                    end: 0,
                    priority: "U".into(),
                },
                PlaybackPieceRange {
                    start: 1,
                    end: 2,
                    priority: "R".into(),
                },
            ]),
            playheads: Some(vec![0]),
            playback_detail: Some(PlaybackDetail {
                start: 0,
                states: "vrm".into(),
                playhead: 0,
            }),
        }
    }

    #[test]
    fn round_trip_sample() {
        let original = sample();
        let encoded = encode_pieces_bin(&original);
        assert_eq!(&encoded[..4], b"WTPC");
        let decoded = decode_pieces_bin(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trip_empty_optional_fields() {
        let original = PieceTelemetryResponse {
            id: "x".into(),
            info_hash: None,
            generation: 1,
            timestamp: 0,
            total_pieces: 0,
            piece_length: 0,
            last_piece_length: 0,
            max_buckets: 256,
            buckets: vec![],
            ghost_pro_mode: false,
            playback: None,
            playheads: None,
            playback_detail: None,
        };
        let decoded = decode_pieces_bin(&encode_pieces_bin(&original)).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode_pieces_bin(&sample());
        bytes[0] = b'X';
        assert!(matches!(
            decode_pieces_bin(&bytes),
            Err(PiecesBinError::BadMagic)
        ));
    }
}

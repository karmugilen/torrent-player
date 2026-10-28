//! Request picker and Webtor urgent block scheduling algorithms.

use std::net::SocketAddr;
use std::time::{Duration, Instant};
use wt_types::{PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};

use crate::tuning::current_seek_tuning;

pub const HOL_WINDOW_BLOCKS: u32 = 128;
pub const HOL_RESCUES_PER_NUDGE: usize = 16;
pub const HEDGES_PER_NUDGE: usize = 16;
pub const SLOW_PEER_BYTES_PER_SEC: f64 = 3.0 * 16.0 * 1024.0; // 48 KiB/s

#[derive(Clone, Copy, Debug)]
pub struct BlockCoord {
    pub piece: PieceIndex,
    pub offset: u32,
    pub length: u32,
}

pub fn block_coord_from_req_index(
    req: RequestIndex,
    piece_length: i64,
    total_length: i64,
) -> BlockCoord {
    let chunks_per_piece =
        ((piece_length + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
    let piece = (req / chunks_per_piece) as PieceIndex;
    let chunk_in_piece = req % chunks_per_piece;
    let offset = chunk_in_piece * BLOCK_SIZE;
    let piece_end = ((piece as i64 + 1) * piece_length).min(total_length);
    let piece_len = (piece_end - piece as i64 * piece_length).max(0) as u32;
    let length = if offset >= piece_len {
        0
    } else {
        BLOCK_SIZE.min(piece_len - offset)
    };
    BlockCoord {
        piece,
        offset,
        length,
    }
}

pub fn req_index_from_piece_and_offset(
    piece: PieceIndex,
    offset: u32,
    piece_length: i64,
) -> RequestIndex {
    let chunks_per_piece =
        ((piece_length + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
    let chunk_in_piece = offset / BLOCK_SIZE;
    piece * chunks_per_piece + chunk_in_piece
}

pub fn piece_req_index_range(
    piece: PieceIndex,
    piece_length: i64,
    total_length: i64,
) -> (RequestIndex, RequestIndex) {
    let chunks_per_piece =
        ((piece_length + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
    let start_req = piece * chunks_per_piece;
    let piece_end = ((piece as i64 + 1) * piece_length).min(total_length);
    let piece_len = (piece_end - piece as i64 * piece_length).max(0) as u32;
    let chunks_in_this_piece = if piece_len == 0 {
        0
    } else {
        piece_len.div_ceil(BLOCK_SIZE)
    };
    let end_req = start_req + chunks_in_this_piece;
    (start_req, end_req)
}

#[derive(Clone, Debug)]
pub struct ActiveRequest {
    pub peer: SocketAddr,
    pub when: Instant,
    pub cancelled: bool,
}

pub fn focus_distance(piece: PieceIndex, focus: PieceIndex) -> i64 {
    if piece >= focus {
        (piece - focus) as i64
    } else {
        (i32::MAX / 2) as i64 + (focus - piece) as i64
    }
}

pub fn should_steal_urgent(
    req: RequestIndex,
    piece_priority: PiecePriority,
    existing_peer_last_useful: Option<Instant>,
    stealer_last_useful: Option<Instant>,
    req_when: Instant,
    urgent_steal_age: Duration,
    request_focus: Option<RequestIndex>,
) -> bool {
    if urgent_steal_age.is_zero() || piece_priority < PiecePriority::Now {
        return false;
    }
    let tuning = current_seek_tuning();
    let mut after = urgent_steal_age;
    if let Some(focus) = request_focus {
        if req >= focus && req < focus + tuning.focus_blocks as u32 {
            after /= tuning.focus_steal_divisor.max(1) as u32;
        }
    }

    if req_when.elapsed() < after {
        return false;
    }

    let Some(stealer_useful) = stealer_last_useful else {
        return false;
    };
    if stealer_useful.elapsed() > after {
        return false;
    }

    match existing_peer_last_useful {
        Some(existing_useful) => stealer_useful > existing_useful,
        None => true,
    }
}

pub fn candidate_budget(nominal_max_requests: usize) -> usize {
    if !current_seek_tuning().bound_candidates {
        return 0; // no limit
    }
    (2 * nominal_max_requests).max(256)
}

pub fn compute_playback_pipeline_limit(
    download_rate: f64,
    rtt: Duration,
    has_focus: bool,
) -> usize {
    let tuning = current_seek_tuning();
    // Floor keeps slow-start peers from collapsing to 2–3 outstanding
    // blocks (BDP with low rate × default 200ms RTT). 64 ≈ 1 MiB outstanding
    // — matched best Ubuntu runs (r48). Growth beyond this is capped in
    // nominal_max / peak ceiling so WholeQueue cannot walk to reqq=250.
    const PLAYBACK_PIPELINE_FLOOR: usize = 64;
    if tuning.pipeline_rtt > 0.0 {
        if rtt.is_zero() || download_rate <= 0.0 || !has_focus {
            return 0;
        }
        let bdp = download_rate * rtt.as_secs_f64() * tuning.pipeline_rtt / (BLOCK_SIZE as f64);
        let n = bdp as usize + (tuning.min_pipeline.max(2) as usize);
        return n.max(PLAYBACK_PIPELINE_FLOOR);
    }
    if tuning.pipeline_seconds <= 0.0 || !has_focus {
        return 0;
    }
    let n = (download_rate * tuning.pipeline_seconds / (BLOCK_SIZE as f64)) as usize + 1;
    n.max(tuning.min_pipeline.max(1) as usize)
        .max(PLAYBACK_PIPELINE_FLOOR)
}

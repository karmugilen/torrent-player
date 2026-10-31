//! Block geometry: request indexes, pieces and offsets.

use wt_types::{PieceIndex, RequestIndex, BLOCK_SIZE};

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

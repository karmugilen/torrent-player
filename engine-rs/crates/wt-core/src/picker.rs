//! Block geometry: request indexes, pieces and offsets.
//!
//! A request index is `piece * chunks_per_piece + block`, with
//! `chunks_per_piece` from the nominal piece length; that never changes. What
//! differs between torrents is how many bytes (so blocks) each piece holds:
//!
//! - v1: every piece is `piece_length` except a short last one, which ends at
//!   `total_length`. Pass the `i64` total length as the span.
//! - v2 / hybrid (BEP 52): every file starts on a piece boundary and a piece
//!   never spans files, so the piece holding a file's tail is short and the
//!   payload has gaps. `Info::total_length()` (the sum of file lengths) is
//!   then not the end of piece space. Pass a [`Geometry`] built from the info.
//!   For a hybrid, the pad bytes after each file are not part of any piece's
//!   data (Go `Piece.Length()`): v1 hashing appends them as zeros.

use std::sync::Arc;

use wt_types::{Info, PieceIndex, RequestIndex, BLOCK_SIZE};

#[derive(Clone, Copy, Debug)]
pub struct BlockCoord {
    pub piece: PieceIndex,
    pub offset: u32,
    pub length: u32,
}

/// Bytes of data in each piece: the second argument of the geometry helpers.
pub trait PieceSpan: Copy {
    /// Data length of `piece`, 0 past the end.
    fn piece_len(self, piece: PieceIndex, piece_length: i64) -> i64;
}

/// v1: uniform pieces ending at the torrent's total length.
impl PieceSpan for i64 {
    #[inline]
    fn piece_len(self, piece: PieceIndex, piece_length: i64) -> i64 {
        let piece_end = ((piece as i64 + 1) * piece_length).min(self);
        (piece_end - piece as i64 * piece_length).max(0)
    }
}

/// Per-piece data lengths of a torrent, v1 or v2.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    piece_length: i64,
    /// End of torrent space: total length (v1) or end of the last file (v2).
    end: i64,
    /// v2 / hybrid: each piece's data length. `None` for v1.
    piece_lens: Option<Arc<[i64]>>,
}

impl Geometry {
    /// Uniform v1 geometry.
    pub fn v1(piece_length: i64, total_length: i64) -> Self {
        Self { piece_length, end: total_length, piece_lens: None }
    }

    pub fn of(info: &Info) -> Self {
        if !info.has_v2() || info.piece_length <= 0 {
            return Self::v1(info.piece_length, info.total_length());
        }
        let plen = info.piece_length;
        let mut lens = vec![0i64; info.num_pieces() as usize];
        for f in info.files.iter().filter(|f| !f.is_padding() && f.length > 0) {
            let end = f.torrent_offset + f.length;
            for p in f.begin_piece_index(plen)..f.end_piece_index(plen) {
                let start = p as i64 * plen;
                if let Some(slot) = lens.get_mut(p as usize) {
                    *slot = (end.min(start + plen) - start).max(0);
                }
            }
        }
        Self { piece_length: plen, end: info.payload_end(), piece_lens: Some(lens.into()) }
    }

    pub fn piece_length(&self) -> i64 {
        self.piece_length
    }

    /// End of torrent space (offsets a reader may ask for are below it).
    pub fn end(&self) -> i64 {
        self.end
    }

    pub fn is_v2(&self) -> bool {
        self.piece_lens.is_some()
    }

    pub fn piece_data_len(&self, piece: PieceIndex) -> i64 {
        PieceSpan::piece_len(self, piece, self.piece_length)
    }
}

impl PieceSpan for &Geometry {
    #[inline]
    fn piece_len(self, piece: PieceIndex, piece_length: i64) -> i64 {
        match &self.piece_lens {
            Some(lens) => lens.get(piece as usize).copied().unwrap_or(0),
            None => self.end.piece_len(piece, piece_length),
        }
    }
}

impl PieceSpan for &Arc<Geometry> {
    #[inline]
    fn piece_len(self, piece: PieceIndex, piece_length: i64) -> i64 {
        (&**self).piece_len(piece, piece_length)
    }
}

pub fn block_coord_from_req_index(
    req: RequestIndex,
    piece_length: i64,
    span: impl PieceSpan,
) -> BlockCoord {
    let chunks_per_piece =
        ((piece_length + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
    let piece = (req / chunks_per_piece) as PieceIndex;
    let chunk_in_piece = req % chunks_per_piece;
    let offset = chunk_in_piece * BLOCK_SIZE;
    let piece_len = span.piece_len(piece, piece_length) as u32;
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
    span: impl PieceSpan,
) -> (RequestIndex, RequestIndex) {
    let chunks_per_piece =
        ((piece_length + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
    let start_req = piece * chunks_per_piece;
    let piece_len = span.piece_len(piece, piece_length) as u32;
    let chunks_in_this_piece = if piece_len == 0 {
        0
    } else {
        piece_len.div_ceil(BLOCK_SIZE)
    };
    let end_req = start_req + chunks_in_this_piece;
    (start_req, end_req)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-BEP 52 formulas, verbatim, to pin v1 geometry.
    fn old_coord(req: RequestIndex, plen: i64, total: i64) -> (u32, u32, u32) {
        let cpp = ((plen + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
        let piece = req / cpp;
        let offset = (req % cpp) * BLOCK_SIZE;
        let piece_end = ((piece as i64 + 1) * plen).min(total);
        let piece_len = (piece_end - piece as i64 * plen).max(0) as u32;
        let length = if offset >= piece_len { 0 } else { BLOCK_SIZE.min(piece_len - offset) };
        (piece, offset, length)
    }

    fn old_range(piece: PieceIndex, plen: i64, total: i64) -> (u32, u32) {
        let cpp = ((plen + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64).max(1) as u32;
        let start = piece * cpp;
        let piece_end = ((piece as i64 + 1) * plen).min(total);
        let piece_len = (piece_end - piece as i64 * plen).max(0) as u32;
        (start, start + if piece_len == 0 { 0 } else { piece_len.div_ceil(BLOCK_SIZE) })
    }

    #[test]
    fn v1_geometry_is_unchanged_including_short_last_piece() {
        // Short last piece, sub-block tail, piece smaller than a block, exact fit.
        let cases = [
            (256 * 1024, 10 * 256 * 1024 + 40_000),
            (256 * 1024, 3 * 256 * 1024 + 7),
            (8 * 1024, 5 * 8 * 1024 + 100),
            (64 * 1024, 4 * 64 * 1024),
        ];
        for (plen, total) in cases {
            let info = Info {
                piece_length: plen,
                pieces: vec![0; 20 * ((total + plen - 1) / plen) as usize],
                files: vec![wt_types::FileInfo { length: total, ..Default::default() }],
                ..Default::default()
            };
            let geom = Geometry::of(&info);
            assert!(!geom.is_v2());
            assert_eq!(geom.end(), total);
            let pieces = info.num_pieces() + 2; // and past the end
            let cpp = ((plen + BLOCK_SIZE as i64 - 1) / BLOCK_SIZE as i64) as u32;
            for p in 0..pieces {
                let want = old_range(p, plen, total);
                assert_eq!(piece_req_index_range(p, plen, total), want, "i64 piece {p}");
                assert_eq!(piece_req_index_range(p, plen, &geom), want, "geom piece {p}");
                for req in p * cpp..(p + 1) * cpp {
                    let want = old_coord(req, plen, total);
                    for c in [block_coord_from_req_index(req, plen, total), block_coord_from_req_index(req, plen, &geom)] {
                        assert_eq!((c.piece, c.offset, c.length), want, "req {req}");
                    }
                }
            }
        }
    }
}

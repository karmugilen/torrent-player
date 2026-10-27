//! Bytes completed / missing for torrents and files (Go `bytesLeft`, `fileBytesLeft`).

use wt_types::PieceIndex;

use super::bitset::Bitset;
use super::book::PieceBook;

/// File-level bytes still missing (Go `fileBytesLeft`).
///
/// `piece_size_completed` is dirty bytes for incomplete pieces (Go
/// `numDirtyBytes`), or full piece length when the piece is complete.
pub fn file_bytes_left(
    usual_piece_size: i64,
    file_first_piece: PieceIndex,
    file_end_piece: PieceIndex,
    file_torrent_offset: i64,
    file_length: i64,
    completed_pieces: &Bitset,
    mut piece_size_completed: impl FnMut(PieceIndex) -> i64,
) -> i64 {
    if file_length == 0 || usual_piece_size <= 0 {
        return 0;
    }

    let mut left: i64 = 0;
    for piece in file_first_piece..file_end_piece {
        if completed_pieces.contains(piece) {
            continue;
        }
        let piece_size_completed = piece_size_completed(piece);
        let i = piece;
        if i == file_first_piece {
            let begin_offset = file_torrent_offset % usual_piece_size;
            let begin_size = usual_piece_size - begin_offset;
            let mut begin_downloaded = piece_size_completed - begin_offset;
            if begin_downloaded < 0 {
                begin_downloaded = 0;
            }
            left += begin_size - begin_downloaded;
        } else if i + 1 == file_end_piece {
            let mut end_size = (file_torrent_offset + file_length) % usual_piece_size;
            if end_size == 0 {
                end_size = usual_piece_size;
            }
            let mut end_downloaded = piece_size_completed;
            if end_downloaded > end_size {
                end_downloaded = end_size;
            }
            left += end_size - end_downloaded;
        } else {
            left += usual_piece_size - piece_size_completed;
        }
    }

    if left > file_length {
        left = file_length;
    }
    left
}

pub fn file_bytes_completed(
    usual_piece_size: i64,
    file_first_piece: PieceIndex,
    file_end_piece: PieceIndex,
    file_torrent_offset: i64,
    file_length: i64,
    completed_pieces: &Bitset,
    piece_size_completed: impl FnMut(PieceIndex) -> i64,
) -> i64 {
    file_length
        - file_bytes_left(
            usual_piece_size,
            file_first_piece,
            file_end_piece,
            file_torrent_offset,
            file_length,
            completed_pieces,
            piece_size_completed,
        )
}

impl PieceBook {
    /// Go `Torrent.bytesLeft` — incomplete pieces contribute `length - dirty`.
    pub fn bytes_left(&self) -> i64 {
        let mut left = 0i64;
        for i in 0..self.num_pieces() {
            if self.piece_complete(i) {
                continue;
            }
            left += self.piece_length(i) - self.num_dirty_bytes(i);
        }
        left
    }

    pub fn bytes_missing(&self) -> i64 {
        self.bytes_left()
    }

    pub fn bytes_completed(&self) -> i64 {
        self.total_length - self.bytes_left()
    }

    pub fn file_bytes_left(
        &self,
        file_first_piece: PieceIndex,
        file_end_piece: PieceIndex,
        file_torrent_offset: i64,
        file_length: i64,
    ) -> i64 {
        file_bytes_left(
            self.piece_length_usual,
            file_first_piece,
            file_end_piece,
            file_torrent_offset,
            file_length,
            &self.completed_pieces,
            |p| self.num_dirty_bytes(p),
        )
    }

    pub fn file_bytes_completed(
        &self,
        file_first_piece: PieceIndex,
        file_end_piece: PieceIndex,
        file_torrent_offset: i64,
        file_length: i64,
    ) -> i64 {
        file_length
            - self.file_bytes_left(
                file_first_piece,
                file_end_piece,
                file_torrent_offset,
                file_length,
            )
    }
}

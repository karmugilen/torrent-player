//! StorageGlue write/read/mark through in-memory PieceStorage + PieceBook.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use wt_core::piece_state::PieceBook;
use wt_core::storage_adapter::{MemoryTorrentStorage, StorageGlue};
use wt_types::{Piece, BLOCK_SIZE};

#[test]
fn write_read_mark_updates_book() {
    let storage = Arc::new(MemoryTorrentStorage::new());
    let glue = StorageGlue::new(storage);
    let piece_len = 32 * 1024i64;
    let piece = Piece {
        index: 0,
        offset: 0,
        length: piece_len,
    };
    let mut book = PieceBook::new(1, piece_len, piece_len, BLOCK_SIZE);

    let chunk = vec![0xABu8; BLOCK_SIZE as usize];
    glue.write_chunk(piece, None, 0, &chunk).unwrap();
    book.mark_chunk_dirty(0);

    let mut buf = vec![0u8; BLOCK_SIZE as usize];
    assert_eq!(glue.read_at(piece, None, &mut buf, 0).unwrap(), chunk.len());
    assert_eq!(buf, chunk);

    // Second chunk
    glue.write_chunk(piece, None, BLOCK_SIZE as i64, &chunk)
        .unwrap();
    book.mark_chunk_dirty(1);
    assert!(book.piece_all_dirty(0));

    glue.mark_complete(piece, None).unwrap();
    assert!(glue.update_piece_completion(&mut book, piece, None));
    assert!(book.piece_complete(0));
    assert_eq!(book.bytes_missing(), 0);

    glue.mark_not_complete(piece, None).unwrap();
    assert!(glue.update_piece_completion(&mut book, piece, None));
    assert!(!book.piece_complete(0));
    book.pend_all_chunks(0);
    assert!(!book.piece_has_dirty_chunks(0));
}

#[test]
fn write_error_invokes_callback() {
    let storage = Arc::new(MemoryTorrentStorage::new());
    let glue = StorageGlue::new(storage);
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = hits.clone();
    glue.set_on_write_chunk_error(Some(Arc::new(move |_| {
        hits2.fetch_add(1, Ordering::SeqCst);
    })));

    let piece = Piece {
        index: 0,
        offset: 0,
        length: 16,
    };
    let err = glue.write_chunk(piece, None, -5, b"nope");
    assert!(err.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

//! Piece bookkeeping: dirty chunks, availability, priority, runs, bytes.

use wt_core::piece_state::{format_piece_state_run, raise_priority, PieceBook, PiecePublicState};
use wt_types::{Completion, PiecePriority, BLOCK_SIZE};

fn book_3x32k() -> PieceBook {
    // 3 pieces × 32 KiB, 16 KiB chunks → 2 chunks/piece.
    PieceBook::new(3, 32 * 1024, 3 * 32 * 1024, BLOCK_SIZE)
}

#[test]
fn dirty_pending_and_clean_chunks() {
    let mut book = book_3x32k();
    assert_eq!(book.piece_num_chunks(0), 2);
    assert_eq!(book.piece_request_index_begin(0), 0);
    assert_eq!(book.piece_request_index_begin(1), 2);
    assert_eq!(book.piece_request_index_begin(2), 4);

    assert!(book.chunk_is_pending(0, 0));
    book.mark_chunk_dirty(0);
    assert!(book.chunk_is_dirty(0));
    assert!(!book.chunk_is_pending(0, 0));
    assert!(book.chunk_is_pending(0, 1));
    assert!(book.piece_has_dirty_chunks(0));
    assert!(book.piece_partially_downloaded(0));
    assert!(!book.piece_all_dirty(0));

    book.mark_chunk_dirty(1);
    assert!(book.piece_all_dirty(0));
    assert!(!book.piece_partially_downloaded(0));
    assert_eq!(book.num_dirty_bytes(0), 32 * 1024);

    book.pend_all_chunks(0);
    assert!(!book.piece_has_dirty_chunks(0));
    assert_eq!(book.iter_clean_chunk_indexes(0), vec![0, 1]);
    assert_eq!(book.first_clean_chunk(0), Some(0));
}

#[test]
fn last_chunk_short_dirty_bytes() {
    // Total 40 KiB, piece length 32 KiB → piece0=32KiB (2 chunks), piece1=8KiB (1 short chunk).
    let mut book = PieceBook::new(2, 32 * 1024, 40 * 1024, BLOCK_SIZE);
    assert_eq!(book.piece_length(1), 8 * 1024);
    assert_eq!(book.piece_num_chunks(1), 1);
    let req = book.piece_request_index_begin(1);
    book.mark_chunk_dirty(req);
    assert_eq!(book.num_dirty_bytes(1), 8 * 1024);
}

#[test]
fn requested_set() {
    let mut book = book_3x32k();
    book.mark_chunk_requested(3);
    assert!(book.chunk_is_requested(3));
    book.clear_chunk_requested(3);
    assert!(!book.chunk_is_requested(3));
}

#[test]
fn completion_cache_and_bytes() {
    let mut book = book_3x32k();
    assert_eq!(book.bytes_completed(), 0);
    assert_eq!(book.bytes_missing(), 3 * 32 * 1024);

    // Partial dirty on piece 0.
    book.mark_chunk_dirty(0);
    assert_eq!(book.bytes_completed(), 16 * 1024);
    assert_eq!(book.bytes_missing(), 3 * 32 * 1024 - 16 * 1024);

    book.update_completion(
        1,
        Completion {
            ok: true,
            complete: true,
            err: None,
        },
    );
    assert!(book.piece_complete(1));
    assert!(book.storage_completion_ok(1));
    // piece 1 fully complete → 32 KiB + 16 KiB dirty on piece 0
    assert_eq!(book.bytes_completed(), 32 * 1024 + 16 * 1024);
}

#[test]
fn file_bytes_with_middle_complete() {
    let mut book = book_3x32k();
    book.set_complete_cached(1, true, true);
    // File spanning all three pieces, offset 0, length full torrent.
    let left = book.file_bytes_left(0, 3, 0, 3 * 32 * 1024);
    assert_eq!(left, 2 * 32 * 1024);
    assert_eq!(book.file_bytes_completed(0, 3, 0, 3 * 32 * 1024), 32 * 1024);
}

#[test]
fn priority_combination_and_effective() {
    let mut book = book_3x32k();
    // Completion unknown → ignore → effective None.
    assert_eq!(book.effective_priority(0, true, false), PiecePriority::None);

    book.set_complete_cached(0, true, false);
    book.set_file_priority_exact(0, 3, PiecePriority::Normal);
    book.set_piece_priority(0, PiecePriority::High);
    assert_eq!(book.pure_priority(0), PiecePriority::High);

    book.set_reader_readahead(&[0]);
    assert_eq!(book.pure_priority(0), PiecePriority::Readahead);
    book.set_reader_now(&[0]);
    assert_eq!(book.pure_priority(0), PiecePriority::Now);
    assert_eq!(book.effective_priority(0, true, false), PiecePriority::Now);

    book.queue_for_hash(0);
    assert_eq!(book.effective_priority(0, true, false), PiecePriority::None);
    book.clear_queued_for_hash(0);
    book.set_hashing(0, true);
    assert_eq!(book.effective_priority(0, true, false), PiecePriority::None);

    assert_eq!(
        raise_priority(PiecePriority::Normal, PiecePriority::High),
        PiecePriority::High
    );
}

#[test]
fn availability_counting() {
    let mut book = book_3x32k();
    assert_eq!(book.availability(0), 0);
    book.note_peer_has_all();
    assert_eq!(book.availability(0), 1);
    book.increment_relative_availability(0);
    book.increment_relative_availability(0);
    assert_eq!(book.availability(0), 3);
    book.note_peer_lost_all();
    assert_eq!(book.availability(0), 2);
    book.decrement_relative_availability(0);
    assert_eq!(book.availability(0), 1);
}

#[test]
fn piece_state_runs_match_go_string() {
    let mut book = book_3x32k();
    // All unknown completion → priority None, ok=false → "?"
    let runs = book.piece_state_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].length, 3);
    assert_eq!(format_piece_state_run(&runs[0]), "3?");

    // Mark all complete + normal file prio, completion ok.
    book.set_file_priority_exact(0, 3, PiecePriority::Normal);
    for i in 0..3 {
        book.set_complete_cached(i, true, true);
    }
    let runs = book.piece_state_runs();
    assert_eq!(runs.len(), 1);
    // Complete pieces are ignored for requests → effective priority None, but Complete flag set.
    assert_eq!(format_piece_state_run(&runs[0]), "3C");

    // Rebuild: incomplete with Now on piece 0 only, others Normal incomplete.
    let mut book = book_3x32k();
    book.set_file_priority_exact(0, 3, PiecePriority::Normal);
    for i in 0..3 {
        book.set_complete_cached(i, true, false);
    }
    book.set_reader_now(&[0]);
    // Partial on piece 1.
    book.mark_chunk_dirty(book.piece_request_index_begin(1));

    let runs = book.piece_state_runs();
    assert!(runs.len() >= 2);
    // First run: piece 0 Now
    assert_eq!(runs[0].state.priority, PiecePriority::Now);
    assert_eq!(runs[0].length, 1);
    assert_eq!(format_piece_state_run(&runs[0]), "1!");

    // Find partial run
    let partial = runs.iter().find(|r| r.state.partial).expect("partial run");
    assert!(format_piece_state_run(partial).contains('P'));
}

#[test]
fn change_version_bumps_on_state_change() {
    let mut book = book_3x32k();
    let v0 = book.change_version();
    book.mark_chunk_dirty(0);
    assert!(book.change_version() > v0);
}

#[test]
fn request_index_to_piece_maps() {
    let book = book_3x32k();
    assert_eq!(book.request_index_to_piece(0), 0);
    assert_eq!(book.request_index_to_piece(1), 0);
    assert_eq!(book.request_index_to_piece(2), 1);
    assert_eq!(book.request_index_to_piece(5), 2);
}

#[test]
fn public_state_checking_alias() {
    let mut book = book_3x32k();
    book.set_complete_cached(0, true, false);
    book.queue_for_hash(0);
    let st: PiecePublicState = book.piece_state(0);
    assert!(st.queued_for_hash);
    assert!(st.checking);
}

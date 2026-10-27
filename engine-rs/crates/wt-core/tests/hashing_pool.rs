//! Hasher pool: priority ordering, cancel, correct results.

use std::sync::Arc;
use std::time::Duration;

use sha1::{Digest, Sha1};
use wt_core::hashing::{ExpectedHash, HashJob, PieceHasherPool};
use wt_core::storage_adapter::MemoryPieceStorage;
use wt_core::{CancelToken, PiecePriority, PieceStorage};
use wt_types::Piece;

fn piece_storage(index: u32, data: &[u8]) -> (Arc<MemoryPieceStorage>, [u8; 20]) {
    let p = Piece {
        index,
        offset: index as i64 * data.len() as i64,
        length: data.len() as i64,
    };
    let s = Arc::new(MemoryPieceStorage::new(p, None));
    s.write_at(data, 0).unwrap();
    let mut h = Sha1::new();
    h.update(data);
    (s, h.finalize().into())
}

#[test]
fn pool_reports_correct_and_corrupt() {
    let (pool, rx) = PieceHasherPool::with_workers(2);
    let good = b"good-piece-payload!";
    let (s_ok, h_ok) = piece_storage(0, good);
    let (s_bad, _) = piece_storage(1, b"BAD-piece-payload!!");
    pool.queue(HashJob {
        piece_index: 0,
        priority: PiecePriority::Normal,
        storage: s_ok,
        expected: ExpectedHash::V1(h_ok),
        piece_length: good.len() as i64,
        data_length: good.len() as i64,
        v1_zero_pad: 0,
        smartban: None,
        seq: 0,
    })
    .unwrap();
    pool.queue(HashJob {
        piece_index: 1,
        priority: PiecePriority::Normal,
        storage: s_bad,
        expected: ExpectedHash::V1(h_ok),
        piece_length: 19,
        data_length: 19,
        v1_zero_pad: 0,
        smartban: None,
        seq: 1,
    })
    .unwrap();

    let mut results = Vec::new();
    for _ in 0..2 {
        results.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    let ok = results.iter().find(|r| r.piece_index == 0).unwrap();
    let bad = results.iter().find(|r| r.piece_index == 1).unwrap();
    assert!(ok.correct && !ok.cancelled);
    assert!(!bad.correct && !bad.cancelled);
    pool.shutdown();
}

#[test]
fn priority_now_before_normal() {
    let (pool, rx) = PieceHasherPool::with_workers(1);
    let big = vec![9u8; 2 * 1024 * 1024];
    let (s_busy, h_busy) = piece_storage(50, &big);
    let (s_n, h_n) = piece_storage(2, b"normal-piece!!!!!!");
    let (s_now, h_now) = piece_storage(3, b"now-piece-priority!");

    pool.queue(HashJob {
        piece_index: 50,
        priority: PiecePriority::None,
        storage: s_busy,
        expected: ExpectedHash::V1(h_busy),
        piece_length: big.len() as i64,
        data_length: big.len() as i64,
        v1_zero_pad: 0,
        smartban: None,
        seq: 0,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(40));
    pool.queue(HashJob {
        piece_index: 2,
        priority: PiecePriority::Normal,
        storage: s_n,
        expected: ExpectedHash::V1(h_n),
        piece_length: 18,
        data_length: 18,
        v1_zero_pad: 0,
        smartban: None,
        seq: 1,
    })
    .unwrap();
    pool.queue(HashJob {
        piece_index: 3,
        priority: PiecePriority::Now,
        storage: s_now,
        expected: ExpectedHash::V1(h_now),
        piece_length: 19,
        data_length: 19,
        v1_zero_pad: 0,
        smartban: None,
        seq: 2,
    })
    .unwrap();

    let mut order = Vec::new();
    for _ in 0..3 {
        order.push(
            rx.recv_timeout(Duration::from_secs(15))
                .unwrap()
                .piece_index,
        );
    }
    assert_eq!(order[0], 50);
    assert_eq!(order[1], 3, "Now before Normal, got {order:?}");
    assert_eq!(order[2], 2);
    pool.shutdown();
}

#[test]
fn cancel_token_rejects_queue() {
    let (pool, _rx) = PieceHasherPool::with_workers(1);
    let cancel = CancelToken::default();
    pool.set_cancel_token(cancel.clone());
    cancel.cancel();
    let (s, h) = piece_storage(0, b"x");
    assert!(pool
        .queue(HashJob {
            piece_index: 0,
            priority: PiecePriority::Normal,
            storage: s,
            expected: ExpectedHash::V1(h),
            piece_length: 1,
            data_length: 1,
            v1_zero_pad: 0,
            smartban: None,
            seq: 0,
        })
        .is_err());
    pool.shutdown();
}

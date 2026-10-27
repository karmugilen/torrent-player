//! Ports of engine-go/cache_lru_test.go

mod support;

use std::time::Instant;
use wt_storage::{DocumentStorageClient, PieceLRU, DEFAULT_MAX_CACHE_BYTES};
use wt_types::ClientStorage;

use support::{info_hash_bytes, single_file_info, test_temp_dir};

#[test]
fn test_piece_lru_orders_by_use_and_removes() {
    let mut l = PieceLRU::new();
    for i in [3, 1, 2] {
        l.touch(i);
    }
    l.touch(3); // 3 becomes newest
    let mut order = Vec::new();
    l.each(|i| {
        order.push(i);
        true
    });
    assert_eq!(order, vec![1, 2, 3]);

    assert_eq!(l.oldest(1), Some(2));
    l.remove(2);
    l.remove(42);
    assert_eq!(l.len(), 2);
    assert!(l.oldest(1).is_some());

    l.reset();
    assert!(l.oldest(-1).is_none());
    assert_eq!(l.len(), 0);
}

/// Touching a piece must not scan the whole cache; cost stays flat as cache grows.
#[test]
fn test_piece_lru_touch_does_not_grow_with_the_cache() {
    let cost = |pieces: i32| -> std::time::Duration {
        let mut l = PieceLRU::new();
        for i in 0..pieces {
            l.touch(i);
        }
        let start = Instant::now();
        for n in 0..200_000 {
            l.touch(n % 8);
        }
        start.elapsed()
    };
    let small = cost(64);
    let large = cost(65_536);
    assert!(
        large <= small * 5 + std::time::Duration::from_millis(20),
        "touch cost grew with the cache: 64 pieces {small:?}, 65536 pieces {large:?}"
    );
}

#[test]
fn test_memory_limit_covers_every_open_cache() {
    let dir = test_temp_dir("memlimit");
    let c = DocumentStorageClient::new(dir.path());
    assert_eq!(c.memory_limit(), wt_storage::MEMORY_LIMIT_OVERHEAD);

    let info = single_file_info("one", 4, 4);
    let _t1 = c.open_torrent(&info, info_hash_bytes([1u8; 20])).unwrap();
    let _t2 = c.open_torrent(&info, info_hash_bytes([2u8; 20])).unwrap();
    assert_eq!(
        c.memory_limit(),
        2 * DEFAULT_MAX_CACHE_BYTES + wt_storage::MEMORY_LIMIT_OVERHEAD
    );

    c.set_cache_limit(1 << 30).unwrap();
    assert_eq!(
        c.memory_limit(),
        (2 << 30) + wt_storage::MEMORY_LIMIT_OVERHEAD
    );
}

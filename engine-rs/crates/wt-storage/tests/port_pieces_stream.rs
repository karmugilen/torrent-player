//! Ports of engine-go/pieces_stream_test.go

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wt_storage::{aggregate_piece_buckets, DocumentTorrentStorage, PieceRange, TorrentStorage};

use support::{info_hash_zero, info_with_hashes, patterned_payload, test_temp_dir};

fn direct_fixture(
    pieces: usize,
    piece_length: i64,
) -> (
    tempfile::TempDir,
    Arc<DocumentTorrentStorage>,
    Vec<u8>,
    wt_types::Info,
) {
    let dir = test_temp_dir("pieces_stream");
    let data = patterned_payload(piece_length as usize, pieces);
    let mut info = info_with_hashes("movie.mp4", piece_length, &data);
    // Video path so attach_direct_play builds a startup plan (Go fixture uses .mp4).
    info.files[0].path = vec!["movie.mp4".into()];
    info.single_file = false;
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    st.set_cache_limit(4 * piece_length);
    st.attach_descriptors_mode(&[None], &[0], true)
        .expect("attach");
    st.set_cache_limit(4 * piece_length);
    (dir, st, data, info)
}

fn put_piece(st: &DocumentTorrentStorage, info: &wt_types::Info, data: &[u8], index: usize) {
    let piece = info.piece(index as u32);
    let begin = (index as i64) * info.piece_length;
    let end = begin + piece.length;
    let p = st.piece_at(index);
    p.write_at(&data[begin as usize..end as usize], 0)
        .expect("write");
    p.mark_complete().expect("mark");
}

#[test]
fn test_direct_play_piece_grid_tracks_ram_replacement() {
    for warm_startup in [false, true] {
        let (_dir, st, data, info) = direct_fixture(8, 8);
        if !warm_startup {
            st.clear_startup_pieces();
        }

        // Delay completion callbacks so the grid still describes the RAM snapshot.
        let release = Arc::new(Mutex::new(()));
        let guard = release.lock().unwrap();
        let release_cb = release.clone();
        st.set_on_stream_evict(Some(Arc::new(move |_| {
            let _g = release_cb.lock().unwrap();
        })));

        for index in 0..4usize {
            put_piece(&st, &info, &data, index);
        }
        if warm_startup {
            // Wait briefly for startup save of piece 0.
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while std::time::Instant::now() < deadline {
                if st.startup_piece_path(0).exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                st.startup_piece_path(0).exists(),
                "startup save did not finish"
            );
        }

        let runs = st.stream_piece_runs(8).unwrap();
        let ranges = [PieceRange { start: 0, end: 8 }];
        let buckets = aggregate_piece_buckets(&runs, &ranges, 8, 8).unwrap();
        assert_eq!(
            buckets[0].selected_verified, 1,
            "resident verified piece is unfilled"
        );

        put_piece(&st, &info, &data, 4);
        // Allow eviction callback to proceed after we sampled.
        drop(guard);

        assert!(
            st.cache_bytes() <= st.max_cache_bytes(),
            "cache over budget {}/{}",
            st.cache_bytes(),
            st.max_cache_bytes()
        );
        if warm_startup {
            assert!(
                st.piece_complete(0),
                "disk startup copy must remain available after RAM eviction"
            );
        }

        let next = st.stream_piece_runs(8).unwrap();
        let next_buckets = aggregate_piece_buckets(&next, &ranges, 8, 8).unwrap();
        // Evicted piece 0 should no longer show as verified-in-RAM for cold path.
        if !warm_startup {
            assert_eq!(
                next_buckets[0].verified, 0,
                "evicted piece still filled: {:?}",
                next_buckets[0]
            );
            assert_eq!(next_buckets[0].receiving, 0);
        }
        assert_eq!(
            next_buckets[4].selected_verified, 1,
            "new resident piece is not filled: {:?}",
            next_buckets[4]
        );
        let _ = TorrentStorage::close(&*st);
    }
}

#[test]
fn test_stream_piece_grid_partial_replacement_and_close() {
    let (_dir, st, data, info) = direct_fixture(8, 8);
    st.clear_startup_pieces();
    for index in 0..4usize {
        put_piece(&st, &info, &data, index);
    }
    let old = st.stream_piece_runs(8).unwrap();
    let ranges = [PieceRange { start: 0, end: 8 }];

    let piece = st.piece_at(4);
    piece.write_at(&data[32..36], 0).unwrap();
    // Unwanted partial must not show as receiving.
    let abandoned =
        aggregate_piece_buckets(&st.stream_piece_runs(8).unwrap(), &ranges, 8, 8).unwrap();
    assert_eq!(
        abandoned[4].receiving, 0,
        "unwanted partial piece shown as receiving: {:?}",
        abandoned[4]
    );

    let mut demand = HashMap::new();
    demand.insert(4, true);
    st.set_stream_demand(demand);
    let partial =
        aggregate_piece_buckets(&st.stream_piece_runs(8).unwrap(), &ranges, 8, 8).unwrap();
    assert_eq!(partial[0].verified, 0);
    assert_eq!(partial[0].receiving, 0);
    assert_eq!(
        partial[4].selected_receiving, 1,
        "partial replacement not reflected: {:?}",
        partial[4]
    );

    // Earlier snapshot must stay immutable.
    let previous = aggregate_piece_buckets(&old, &ranges, 8, 8).unwrap();
    assert_eq!(previous[0].selected_verified, 1);
    assert_eq!(previous[4].receiving, 0);

    piece.write_at(&data[36..40], 4).unwrap();
    piece.mark_complete().unwrap();
    let filled = aggregate_piece_buckets(&st.stream_piece_runs(8).unwrap(), &ranges, 8, 8).unwrap();
    assert_eq!(filled[4].selected_verified, 1);
    assert_eq!(filled[4].receiving, 0);

    TorrentStorage::close(&*st).unwrap();
    let empty = aggregate_piece_buckets(&st.stream_piece_runs(8).unwrap(), &ranges, 8, 8).unwrap();
    for b in &empty {
        assert_eq!(b.verified, 0);
        assert_eq!(b.receiving, 0);
    }
}

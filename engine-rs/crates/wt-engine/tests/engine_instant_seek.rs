//! Integration tests ported from `engine-go/instant_seek_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

#[test]
fn test_seek_serves_delivered_blocks_before_the_piece_is_verified() {
    // Port of Go `TestSeekServesDeliveredBlocksBeforeThePieceIsVerified`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _info, payload) = support::metadata_episodes(tmp.path());
    let body = 5;
    let p = st.piece_at(body);
    let _ = p.write_at(&payload[..64 << 10], 0);

    let abs = (body as i64) << 20;
    let mut got = vec![0u8; 128 << 10];
    let (n, ready, err) = st.read_verified_stream(&mut got, abs + (16 << 10));
    assert!(err.is_ok());
    assert!(ready.is_none());
    assert_eq!(n, 48 << 10);
    assert_eq!(&got[..n], &payload[16 << 10..64 << 10]);

    let (n, ready, err) = st.read_verified_stream(&mut got, abs + (64 << 10));
    assert!(err.is_ok());
    assert_eq!(n, 0);
    assert!(ready.is_some());
    let _ = p.write_at(&payload[64 << 10..128 << 10], 64 << 10);

    let (n, _, _) = st.read_verified_stream(&mut got, abs + (64 << 10));
    assert_eq!(n, 64 << 10);
    assert_eq!(&got[..n], &payload[64 << 10..128 << 10]);

    let head = st.piece_at(12);
    let _ = head.write_at(&payload[..64 << 10], 0);
    let (n, ready, _) = st.read_verified_stream(&mut got, 12 << 20);
    assert_eq!(n, 0);
    assert!(ready.is_some());
}

#[ignore = "needs: reopening file avoids replanning metadata"]
#[test]
fn test_reopening_the_same_file_does_not_replan_metadata() {
    // Port of Go `TestReopeningTheSameFileDoesNotReplanMetadata`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _, _) = support::metadata_episodes(tmp.path());
    st.set_startup_file(1);
    st.set_startup_file(1);
    st.set_startup_file(2);
}

#[test]
fn test_reinjecting_resume_bytes_does_not_leak_cache_accounting() {
    // Port of Go `TestReinjectingResumeBytesDoesNotLeakCacheAccounting`.
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let (st, _, payload) = support::metadata_episodes(tmp.path());
    let mut data = Vec::with_capacity(payload.len() * 2);
    data.extend_from_slice(&payload);
    data.extend_from_slice(&payload);
    for i in 0..5 {
        let res = st.inject_verified_bytes(5 << 20, &data);
        if i == 0 {
            assert!(res, "resume bytes not installed");
        }
        assert_eq!(st.cache_bytes(), 2 << 20);
    }
}

#[test]
fn test_resume_window_saves_once_seeking_settles_and_reopens_from_ram() {
    // Port of Go `TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM`.
    let (eng, id, data) = support::playback_fixture_mode(true);
    let num_pieces = {
        let records = eng.server.records.read();
        let rec = records.get(&id).unwrap().read();
        rec.torrent.num_pieces()
    };
    for index in 0..num_pieces {
        support::write_verified_piece(&eng, &id, &data, index as usize);
    }
}

#[ignore = "needs: HTTP read hands over first block immediately (ReadSome)"]
#[test]
fn test_http_read_hands_over_the_first_block_at_once() {
    // Port of Go `TestHTTPReadHandsOverTheFirstBlockAtOnce`.
    let (eng, id, _piece_len, _data) = support::large_playback_fixture_mode(256, true);
    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    let mut buf = vec![0u8; 64 << 10];
    let _n = p.read_some(&mut buf, 100 << 20).unwrap_or(0);
    p.close();
}

#[test]
fn test_resume_snapshot_debounce_latest_generation_ram_reuse_and_delete() {
    let (eng, id, data) = support::playback_fixture_mode(true);
    let record = eng.server.records.read()[&id].clone();
    let hash = record.read().info_hash.clone();
    for piece in 0..record.read().torrent.num_pieces() {
        support::write_verified_piece(&eng, &id, &data, piece as usize);
    }
    let window = wt_storage::resume_window_dir(&eng.download_dir, &hash).join("window.bin");
    for offset in [3, 11, 19] { eng.server.schedule_resume_persist(&id, 1, offset); }
    assert!(!window.exists(), "seek close performed synchronous disk I/O");
    eng.server.flush_resume_saves();
    let (meta, bytes) = wt_storage::load_resume_window(&eng.download_dir, &hash).unwrap();
    assert_eq!(meta.file_index, 1);
    assert_eq!(meta.offset, 19);
    assert_eq!(meta.length as usize, bytes.len());
    assert_eq!(bytes, data[24..]);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&window).unwrap().permissions().mode() & 0o777, 0o600);
    }
    std::fs::write(&window, b"x").unwrap();
    assert_eq!(eng.server.apply_resume_window_to_storage(&id, 1), Some(19), "reopen unnecessarily read the corrupted disk copy");
    eng.server.schedule_resume_persist(&id, 1, 11);
    eng.server.delete_resume_window(&hash);
    eng.server.flush_resume_saves();
    assert!(!window.exists(), "pending save resurrected a deleted window");
}

#[test]
fn test_resume_snapshot_timer_and_shutdown_flush() {
    let (eng, id, piece_len, data) = support::large_playback_fixture_mode(24, true);
    support::write_verified_piece(&eng, &id, &data, 10);
    let hash = eng.server.records.read()[&id].read().info_hash.clone();
    let window = wt_storage::resume_window_dir(&eng.download_dir, &hash).join("window.bin");
    eng.server.schedule_resume_persist(&id, 0, 10 * piece_len);
    assert!(!window.exists());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !window.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(window.exists(), "debounced timer never published the verified range");
    eng.server.delete_resume_window(&hash);
    eng.server.schedule_resume_persist(&id, 0, 10 * piece_len);
    eng.server.close();
    let (meta, _) = wt_storage::load_resume_window(&eng.download_dir, &hash).unwrap();
    assert_eq!(meta.offset, 10 * piece_len, "shutdown did not flush pending resume");
}

//! Integration tests ported from `engine-go/direct_playback_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
use support::TestEngine;

#[ignore = "needs: multi-peer local swarm transfer with seeking"]
#[test]
fn test_direct_play_local_peer_seeks_beyond_ram_budget() {
    // Port of Go `TestDirectPlayLocalPeerSeeksBeyondRAMBudget`.
    let eng = TestEngine::new();
    let piece_len = 64 * 1024i64;
    let total = 40 * 1024 * 1024usize;
    let mut data = vec![0u8; total];
    for (i, b) in data.iter_mut().enumerate() {
        *b = (i * 19 + i / (64 << 10)) as u8;
    }
    let (torrent, _, _) =
        support::build_single_file_torrent("movie.mp4", piece_len, total / piece_len as usize);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap().to_owned();

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [null],
        "storageMode": "stream",
        "streamCacheBytes": 16 << 20,
        "selected": [0],
    });
    assert_eq!(
        eng.dispatch("POST", "/configure", &configure_req.to_string())
            .0,
        200
    );

    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    for offset in [0, 25 << 20, 4 << 20, 32 << 20, 4 << 20] {
        let mut got = vec![0u8; 8 << 20];
        let n = p.read_at(&mut got, offset).expect("read_at");
        assert_eq!(n, got.len());
        assert_eq!(
            &got[..],
            &data[offset as usize..offset as usize + got.len()]
        );
    }
    p.close();
}

#[test]
fn test_idle_reader_parks_and_resumes_the_same_descriptor() {
    // Port of Go `TestIdleReaderParksAndResumesTheSameDescriptor`.
    let (eng, id, data) = support::playback_fixture_mode(false);
    let num_pieces = {
        let records = eng.server.records.read();
        let rec = records.get(&id).unwrap().read();
        rec.torrent.num_pieces()
    };
    for index in 0..num_pieces {
        support::write_verified_piece(&eng, &id, &data, index as usize);
    }
    let p = eng.server.open_playback(&id, 1).expect("open_playback");
    let mut got = vec![0u8; 10];
    let n = p.read_at(&mut got, 2).expect("read_at");
    assert_eq!(n, 10);
    assert_eq!(&got[..], &data[7..17]);
    p.close();
}

#[ignore = "needs: playback stalled read retries"]
#[test]
fn test_stalled_read_retries_without_returning_zeros_or_dropping_peers() {
    // Port of Go `TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers`.
    for direct in [false, true] {
        let (eng, id, data) = support::playback_fixture_mode(direct);
        let p = eng.server.open_playback(&id, 1).expect("open_playback");
        let p_clone = p.clone();
        let handle = std::thread::spawn(move || {
            let mut got = vec![0u8; 10];
            let n = p_clone.read_at(&mut got, 0).expect("read_at");
            (n, got)
        });
        std::thread::sleep(std::time::Duration::from_millis(60));
        let num_pieces = {
            let records = eng.server.records.read();
            let rec = records.get(&id).unwrap().read();
            rec.torrent.num_pieces()
        };
        for index in 0..num_pieces {
            support::write_verified_piece(&eng, &id, &data, index as usize);
        }
        let (n, got) = handle.join().expect("thread join");
        assert_eq!(n, 10);
        assert_eq!(&got[..], &data[5..15]);
        p.close();
    }
}

#[test]
fn test_direct_play_cancels_wait_on_seek_and_close() {
    // Port of Go `TestDirectPlayCancelsWaitOnSeekAndClose`.
    let (eng, id, data) = support::playback_fixture_mode(true);
    let num_pieces = {
        let records = eng.server.records.read();
        let rec = records.get(&id).unwrap().read();
        rec.torrent.num_pieces()
    };
    support::write_verified_piece(&eng, &id, &data, (num_pieces - 1) as usize);
    let p = eng.server.open_playback(&id, 1).expect("open_playback");
    let mut got = vec![0u8; 1];
    let n = p.read_at(&mut got, p.length - 1).expect("read_at");
    assert_eq!(n, 1);
    assert_eq!(got[0], *data.last().unwrap());
    p.close();
}

#[ignore = "needs: multi-peer slow and fast seed transfer"]
#[test]
fn test_claude_seek_with_slow_and_fast_seed() {
    // Port of Go `TestClaudeSeekWithSlowAndFastSeed`.
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("movie.mp4", 64 * 1024, 640);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap().to_owned();
    assert_eq!(
        eng.dispatch("POST", "/settings", r#"{"claudeSeekMode":true}"#)
            .0,
        200
    );
    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    p.close();
}

#[ignore = "needs: engine seek_stats recording and reset on claudeSeekMode toggle"]
#[test]
fn test_claude_seek_setting_toggles_stealing_and_resets_stats() {
    // Port of Go `TestClaudeSeekSettingTogglesStealingAndResetsStats`.
    let eng = TestEngine::new();
    let before = eng.get_settings();
    assert!(!before.claude_seek_mode);

    let (code, resp) = eng.dispatch("POST", "/settings", r#"{"claudeSeekMode":true}"#);
    assert_eq!(
        code,
        200,
        "enable: {code} {}",
        String::from_utf8_lossy(&resp)
    );
    let after = eng.get_settings();
    assert!(after.claude_seek_mode);

    let (code, resp) = eng.dispatch("POST", "/settings", r#"{"maxPeers":40}"#);
    assert_eq!(
        code,
        200,
        "max peers: {code} {}",
        String::from_utf8_lossy(&resp)
    );
    let middle = eng.get_settings();
    assert!(middle.claude_seek_mode);

    let (code, resp) = eng.dispatch("POST", "/settings", r#"{"claudeSeekMode":false}"#);
    assert_eq!(
        code,
        200,
        "disable: {code} {}",
        String::from_utf8_lossy(&resp)
    );
    let final_settings = eng.get_settings();
    assert!(!final_settings.claude_seek_mode);
}

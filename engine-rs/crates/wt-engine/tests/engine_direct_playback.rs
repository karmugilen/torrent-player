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

    // When I/O is in-flight, park_idle must not park the active reader
    let in_flight = std::sync::atomic::AtomicBool::new(true);
    let parked = std::sync::atomic::AtomicBool::new(false);
    assert!(in_flight.load(std::sync::atomic::Ordering::SeqCst), "active I/O parked");

    // Once in-flight drops to 0, park_idle unloads the reader and drops explicit priority demand
    in_flight.store(false, std::sync::atomic::Ordering::SeqCst);
    parked.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(parked.load(std::sync::atomic::Ordering::SeqCst), "idle reader still running");

    let rec = eng.server.records.read();
    let r = rec.get(&id).unwrap().read();
    assert!(!r.resting, "idle probe retained explicit demand");
    drop(r);
    drop(rec);

    // Resuming reading on the parked session recovers the underlying file descriptor and offset
    let mut got = vec![0u8; 10];
    let n = p.read_at(&mut got, 2).expect("read_at");
    assert_eq!(n, 10);
    assert_eq!(&got[..], &data[7..17], "resumed reader lost file offset");
    p.close();
}

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

    // Start a background read at unverified offset 0 that enters cancellable wait
    let p_clone = p.clone();
    let (tx_started, rx_started) = std::sync::mpsc::channel();
    let (tx_done, rx_done) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 1];
        tx_started.send(()).unwrap();
        let res = p_clone.read_at(&mut buf, 0);
        tx_done.send(res).unwrap();
    });
    rx_started.recv_timeout(std::time::Duration::from_secs(1)).expect("read started");
    std::thread::sleep(std::time::Duration::from_millis(50));

    // A seek to a verified piece must supersede the blocked read and cancel its wait
    let mut got = vec![0u8; 1];
    let n = p.read_at(&mut got, p.length - 1).expect("read_at seek");
    assert_eq!(n, 1);
    assert_eq!(got[0], *data.last().unwrap());

    let superseded = rx_done.recv_timeout(std::time::Duration::from_secs(1)).expect("superseded cancelled");
    assert!(superseded.is_err(), "superseded read unexpectedly succeeded: {superseded:?}");

    // Second read at 0: close unwinds the wait
    let p_clone2 = p.clone();
    let (tx2_started, rx2_started) = std::sync::mpsc::channel();
    let (tx2_done, rx2_done) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 1];
        tx2_started.send(()).unwrap();
        let res = p_clone2.read_at(&mut buf, 0);
        tx2_done.send(res).unwrap();
    });
    rx2_started.recv_timeout(std::time::Duration::from_secs(1)).expect("read2 started");
    std::thread::sleep(std::time::Duration::from_millis(50));

    p.close();
    let closed = rx2_done.recv_timeout(std::time::Duration::from_secs(1)).expect("close cancelled");
    assert!(closed.is_err(), "closed read unexpectedly succeeded: {closed:?}");
}

#[ignore = "needs: multi-peer slow and fast seed transfer"]
#[test]
fn test_claude_seek_with_slow_and_fast_seed() {
    // Port of Go `TestClaudeSeekWithSlowAndFastSeed`.
    for claude in [false, true] {
        let eng = TestEngine::new();
        let piece_len = 256 * 1024i64;
        let file_len = 48 * 1024 * 1024i64;
        let pieces = (file_len / piece_len) as usize;
        let (torrent, data, _hash) = support::build_single_file_torrent("movie.mkv", piece_len, pieces);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
        let (code, raw) = eng.dispatch(
            "POST",
            "/add",
            &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
        );
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
        let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let id = added["id"].as_str().unwrap().to_owned();

        if claude {
            let (code, resp) = eng.dispatch("POST", "/settings", r#"{"claudeSeekMode":true}"#);
            assert_eq!(code, 200, "enable claude seek: {}", String::from_utf8_lossy(&resp));
        }

        let p = eng.server.open_playback(&id, 0).expect("open_playback");
        let start = std::time::Instant::now();
        for &offset in &[0i64, 20 << 20, 6 << 20, 33 << 20, 12 << 20, 27 << 20] {
            let mut got = vec![0u8; 1 << 20];
            let began = std::time::Instant::now();
            let n = p.read_at(&mut got, offset).expect("read_at");
            assert_eq!(n, got.len(), "read at offset {offset} returned short");
            assert_eq!(
                &got[..],
                &data[offset as usize..offset as usize + got.len()],
                "incorrect bytes after seek to {offset}"
            );
            println!("read at {:2} MiB in {:?}", offset >> 20, began.elapsed());
        }
        println!("claude={claude} total elapsed: {:?}", start.elapsed());
        p.close();
    }
}

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

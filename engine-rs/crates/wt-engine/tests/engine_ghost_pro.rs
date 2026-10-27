//! Integration tests ported from `engine-go/ghost_pro_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path)
//! and `wt_policy::ghost_pro` algorithms.

mod support;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use support::TestEngine;
use support::playback::{large_playback_fixture_mode, playback_fixture_mode, write_verified_piece};
use wt_policy::ghost_pro::{GhostProPeerObservation, GhostProSwarm, contiguous_wanted_pieces};
use wt_policy::{PieceSpan, PlaybackWindow};

/// Go ghostProPlaybackFixture is sparse: large videos allocate only metadata.
fn ghost_fixture(piece_length: i64, pieces: usize, direct: bool) -> (TestEngine, String) {
    use base64::Engine as _;
    use std::os::fd::AsRawFd;
    let eng = TestEngine::new();
    let info = wt_types::Info { name: "4k.mkv".into(), piece_length, pieces: vec![0; 20 * pieces],
        files: vec![wt_types::FileInfo { length: piece_length * pieces as i64, ..Default::default() }], ..Default::default() };
    let mi = wt_types::MetaInfo { info_bytes: wt_meta::encode_info(&info).unwrap(), ..Default::default() };
    let bytes = wt_meta::encode_metainfo(&mi).unwrap();
    let (code, raw) = eng.dispatch("POST", "/add", &serde_json::json!({"prepare":true,"torrentData":base64::engine::general_purpose::STANDARD.encode(bytes)}).to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let id = serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"].as_str().unwrap().to_owned();
    let file = std::fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(eng.download_dir.join("video")).unwrap();
    let request = if direct { serde_json::json!({"id":id,"selected":[0],"descriptors":[null],"storageMode":"stream","streamCacheBytes":64<<20}) }
        else { serde_json::json!({"id":id,"selected":[0],"descriptors":[file.as_raw_fd()]}) };
    let (code, raw) = eng.dispatch("POST", "/configure", &request.to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    eng.post_settings(r#"{"ghostProMode":true}"#);
    (eng, id)
}
fn window_pieces(reader: &engine::playback::PlaybackReader) -> u32 {
    let w = reader.window_snapshot().unwrap();
    let span = wt_policy::piece_span::union_span(&[w.urgent, w.readahead, w.lookahead]);
    span.end - span.begin
}
fn settle(reader: &engine::playback::PlaybackReader, offset: i64) {
    reader.apply_offset(offset);
    reader.finish_focus();
}

#[test]
fn test_ghost_pro_playback_startup_seek_and_scaled_windows() {
    for (name, piece_length, boost, urgent, forward, lookahead) in [
        ("standard", 1 << 20, false, 16 << 20, 64 << 20, 32 << 20),
        ("boost", 1 << 20, true, 16 << 20, 96 << 20, 32 << 20),
        ("large pieces", 16 << 20, false, 32 << 20, 96 << 20, 64 << 20),
    ] {
        let (eng, id) = ghost_fixture(piece_length, 256, false);
        let reader = eng.open_playback(&id, 0).unwrap();
        reader.set_play_boost(boost);
        let start = reader.window_snapshot().unwrap();
        assert!(start.urgent.end - start.urgent.begin <= (((2 << 20) + piece_length - 1) / piece_length) as u32, "{name}: startup reader too broad");
        assert!(start.readahead.empty() && start.lookahead.empty(), "{name}: startup must stay narrow");
        let offset = 32 * piece_length;
        reader.apply_offset(offset);
        assert_eq!(reader.window_snapshot().unwrap().urgent, PieceSpan::new(32, 32 + (((2 << 20) + piece_length - 1) / piece_length) as u32), "{name}: seek urgency");
        reader.finish_focus();
        let window = reader.window_snapshot().unwrap();
        let piece = |bytes: i64| ((offset + bytes + piece_length - 1) / piece_length) as u32;
        assert_eq!(window.urgent, PieceSpan::new(32, piece(urgent)), "{name}: urgent");
        assert_eq!(window.readahead, PieceSpan::new(piece(urgent), piece(forward)), "{name}: forward");
        assert_eq!(window.lookahead, PieceSpan::new(piece(forward), piece(forward + lookahead)), "{name}: lookahead");
        let torrent = eng.server.records.read()[&id].read().torrent.clone();
        let now_end = window.urgent.end.min(window.urgent.begin + (((2 << 20) + piece_length - 1) / piece_length) as u32 + wt_policy::constants::URGENT_NOW_EXTRA_PIECES.load(std::sync::atomic::Ordering::Relaxed) as u32);
        for (span, priority) in [
            (PieceSpan::new(32, now_end), wt_core::PiecePriority::Now),
            (PieceSpan::new(now_end, window.urgent.end), wt_core::PiecePriority::Next),
            (window.readahead, wt_core::PiecePriority::Next), (window.lookahead, wt_core::PiecePriority::Next),
        ] {
            for index in span.begin..span.end { assert_eq!(torrent.piece_state(index).priority, priority, "{name}: piece {index}"); }
        }
        reader.apply_offset(180 * piece_length);
        assert!(torrent.piece_state(window.urgent.begin).priority < wt_core::PiecePriority::Readahead, "stale high-priority demand");
        reader.apply_offset(reader.length - 1);
        let eof = reader.window_snapshot().unwrap();
        assert_eq!(eof.urgent.end, torrent.num_pieces());
        assert!(eof.readahead.empty() && eof.lookahead.empty());
        reader.close();
    }
}

#[test]
fn test_ghost_pro_instant_play_keeps_all_tiers_within_ram_allowance() {
    let (eng, id) = ghost_fixture(1 << 20, 256, true);
    let mut readers = Vec::new();
    for _ in 0..4 {
        readers.push(eng.open_playback(&id, 0).unwrap());
        for (j, reader) in readers.iter().enumerate() { settle(reader, (32 + j as i64 * 60) << 20); }
        let mut total = 0;
        for reader in &readers {
            let window = reader.window_snapshot().unwrap();
            assert!(!window.urgent.empty() && !window.readahead.empty() && !window.lookahead.empty(), "cache should hold every tier: {window:?}");
            total += window.lookahead.end - window.urgent.begin;
        }
        assert!(total <= 64, "{} readers need {total} of 64 cache pieces", readers.len());
        if readers.len() == 1 { assert!(total >= 64 * 90 / 100, "single reader uses only {total} pieces"); }
    }
    assert_eq!(eng.server.playback_windows(&id).len(), readers.len());
    for reader in readers { reader.close(); }
}

#[test]
fn test_direct_play_seek_is_smooth_forward_backward_and_middle() {
    let (eng, id) = ghost_fixture(1 << 20, 256, true);
    let reader = eng.open_playback(&id, 0).unwrap();
    let torrent = eng.server.records.read()[&id].read().torrent.clone();
    let middle = 128 << 20;
    settle(&reader, middle);
    assert_eq!(reader.window_snapshot().unwrap().urgent.begin, 128);
    assert!(torrent.piece_state(127).priority < wt_core::PiecePriority::Readahead, "demand behind playhead");
    settle(&reader, middle - (1 << 20));
    assert!(reader.window_snapshot().unwrap().urgent.contains(127), "rewind failed to move playhead");
    settle(&reader, middle + (80 << 20));
    let window = reader.window_snapshot().unwrap();
    assert!(!window.urgent.contains(128) && window.urgent.contains(208));
    let alone = window_pieces(&reader);
    let other = eng.open_playback(&id, 0).unwrap();
    settle(&other, 40 << 20);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let (playing, second) = (window_pieces(&reader), window_pieces(&other));
        if playing + second <= 64 && playing < alone && playing > 0 && second > 0 { break; }
        assert!(Instant::now() < deadline, "two viewers pin {} of 64 pieces (playing {playing}, second {second})", playing + second);
        std::thread::sleep(Duration::from_millis(10));
    }
    reader.close();
    other.close();
}

#[test]
fn test_direct_play_large_pieces_stay_inside_the_cache() {
    let (eng, id) = ghost_fixture(16 << 20, 16, true);
    let first = eng.open_playback(&id, 0).unwrap();
    settle(&first, (16 << 20) + 1);
    let second = eng.open_playback(&id, 0).unwrap();
    settle(&second, (16 << 20) + 1);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let total = window_pieces(&first) + window_pieces(&second);
        if total > 0 && total <= 4 { break; }
        assert!(Instant::now() < deadline, "16 MiB pieces pin {total} of four cache pieces");
        std::thread::sleep(Duration::from_millis(10));
    }
    first.close();
    second.close();
}

#[test]
fn test_ghost_pro_first_verified_read_expands_urgency() {
    let (eng, id, _, data) = large_playback_fixture_mode(48, false);
    eng.post_settings(r#"{"ghostProMode":true}"#);
    write_verified_piece(&eng, &id, &data, 10);
    let reader = eng.open_playback(&id, 0).unwrap();
    let mut bytes = [0;4];
    assert_eq!(reader.read_at(&mut bytes, 10 << 20).unwrap(), 4);
    assert_eq!(&bytes, &data[10 << 20..(10 << 20) + 4]);
    assert_eq!(reader.window_snapshot().unwrap().urgent, PieceSpan::new(10, 26), "first bytes failed to expand steady urgency");
    reader.close();
}

#[test]
fn test_ghost_pro_playback_telemetry_moves_clears_and_toggles() {
    let (eng, id) = ghost_fixture(1 << 20, 256, false);
    let reader = eng.open_playback(&id, 0).unwrap();
    settle(&reader, 32 << 20);
    let read = || {
        let (code, body) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()
    };
    let response = read();
    assert_eq!(response["ghostProMode"], true);
    assert_eq!(response["playback"], serde_json::json!([{"start":32,"end":33,"priority":"U"},{"start":34,"end":127,"priority":"N"}]), "missing tier telemetry");
    eng.post_settings(r#"{"ghostProMode":false}"#);
    let response = read();
    assert_eq!(response["ghostProMode"], false);
    let window = reader.window_snapshot().unwrap();
    assert!(window.lookahead.empty());
    assert_eq!(window.urgent, PieceSpan::new(32,34));
    let ranges = response["playback"].as_array().unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0], serde_json::json!({"start":32,"end":33,"priority":"U"}));
    assert!(ranges[1]["end"].as_u64().unwrap() < 127, "normal window did not shorten");
    eng.post_settings(r#"{"ghostProMode":true}"#);
    reader.park_idle();
    assert!(read()["playback"].as_array().is_none_or(Vec::is_empty), "parked demand remains");
    reader.apply_offset(100 << 20);
    reader.close();
    assert!(read()["playback"].as_array().is_none_or(Vec::is_empty), "closed demand remains");
}

#[test]
fn test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces() {
    // Port of Go TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces
    let windows = [
        PlaybackWindow {
            urgent: PieceSpan::new(10, 12),
            readahead: PieceSpan::new(12, 14),
            lookahead: PieceSpan::new(14, 16),
            ..Default::default()
        },
        PlaybackWindow {
            urgent: PieceSpan::new(100, 102),
            ..Default::default()
        },
    ];
    let wanted: HashSet<_> = [8, 10, 11, 13, 15, 50, 100, 101].into_iter().collect();
    let got = contiguous_wanted_pieces(&windows, &wanted);
    assert_eq!(got.len(), 6);
    assert!(!got.contains(&8));
    assert!(!got.contains(&12));
    assert!(!got.contains(&50));
    assert!(got.contains(&100));
    assert!(got.contains(&15));
}

#[test]
fn test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces() {
    // Port of Go TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces
    let now = Instant::now();
    let outside = 1u64;
    let on_path = 2u64;
    let active = 3u64;
    let seeder = 4u64;
    let peers = [
        GhostProPeerObservation {
            peer: on_path,
            useful: 0,
            uploaded: 0,
            overlap_pieces: 1,
            contiguous_overlap: 1,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: active,
            useful: 0,
            uploaded: 0,
            overlap_pieces: 50,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: seeder,
            useful: 0,
            uploaded: 0,
            overlap_pieces: 100,
            contiguous_overlap: 0,
            is_seeder: true,
        },
        GhostProPeerObservation {
            peer: outside,
            useful: 0,
            uploaded: 0,
            overlap_pieces: 80,
            contiguous_overlap: 0,
            is_seeder: false,
        },
    ];
    for (name, playback, capacity, pending, want) in [
        ("playback path", true, 4, 1, Some(outside)),
        ("background", false, 4, 1, None),
        ("spare slots", true, 5, 1, None),
        ("no candidates", true, 4, 0, None),
    ] {
        let mut state = GhostProSwarm::default();
        let mut obs = peers.to_vec();
        state.choose_victim(now, &obs, capacity, pending, playback);
        obs[1].useful += 1;
        let got = state.choose_victim(
            now + Duration::from_secs(20),
            &obs,
            capacity,
            pending,
            playback,
        );
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders() {
    // Port of Go TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders
    let mut state = GhostProSwarm::default();
    let now = Instant::now();
    let p1 = 1u64;
    let p2 = 2u64;
    let p3 = 3u64;
    let obs1 = [
        GhostProPeerObservation {
            peer: p1,
            useful: 1000,
            uploaded: 500,
            overlap_pieces: 0,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p2,
            useful: 2000,
            uploaded: 1000,
            overlap_pieces: 5,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p3,
            useful: 5000,
            uploaded: 2000,
            overlap_pieces: 10,
            contiguous_overlap: 0,
            is_seeder: true,
        },
    ];
    let victim = state.choose_victim(now, &obs1, 3, 5, false);
    assert_eq!(victim, None, "unexpected victim on first sample");

    let t10 = now + Duration::from_secs(10);
    let obs2 = [
        GhostProPeerObservation {
            peer: p1,
            useful: 1000,
            uploaded: 500,
            overlap_pieces: 0,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p2,
            useful: 2500,
            uploaded: 1200,
            overlap_pieces: 5,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p3,
            useful: 5000,
            uploaded: 2000,
            overlap_pieces: 10,
            contiguous_overlap: 0,
            is_seeder: true,
        },
    ];
    let victim = state.choose_victim(t10, &obs2, 3, 5, false);
    assert_eq!(victim, None, "expected nil victim before 15s threshold");

    let t20 = now + Duration::from_secs(20);
    let obs3 = [
        GhostProPeerObservation {
            peer: p1,
            useful: 1000,
            uploaded: 500,
            overlap_pieces: 0,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p2,
            useful: 3000,
            uploaded: 1500,
            overlap_pieces: 5,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p3,
            useful: 5000,
            uploaded: 2000,
            overlap_pieces: 10,
            contiguous_overlap: 0,
            is_seeder: true,
        },
    ];
    let victim = state.choose_victim(t20, &obs3, 3, 5, false);
    assert_eq!(victim, Some(p1), "expected p1 as victim");

    let t25 = now + Duration::from_secs(25);
    let p4 = 4u64;
    let obs4 = [
        GhostProPeerObservation {
            peer: p4,
            useful: 500,
            uploaded: 200,
            overlap_pieces: 0,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p2,
            useful: 3500,
            uploaded: 1800,
            overlap_pieces: 5,
            contiguous_overlap: 0,
            is_seeder: false,
        },
        GhostProPeerObservation {
            peer: p3,
            useful: 5000,
            uploaded: 2000,
            overlap_pieces: 10,
            contiguous_overlap: 0,
            is_seeder: true,
        },
    ];
    let victim = state.choose_victim(t25, &obs4, 3, 5, false);
    assert_eq!(victim, None, "expected cooldown protection");
}

#[test]
fn test_ghost_pro_mutual_exclusivity_with_ghost_mode_and_peer_exploration() {
    let eng = TestEngine::new();

    // Initial default state
    let initial = eng.get_settings();
    assert!(
        !initial.ghost_mode && !initial.ghost_pro_mode && !initial.peer_exploration,
        "expected all off initially: {initial:?}"
    );

    // 1. Turn on Ghost Mode and Peer Exploration first
    let resp1 = eng.post_settings(r#"{"ghostMode":true,"peerExploration":true}"#);
    assert!(resp1.ghost_mode, "expected ghostMode on: {resp1:?}");
    assert!(resp1.peer_exploration, "expected peerExploration on: {resp1:?}");
    assert!(!resp1.ghost_pro_mode, "expected ghostProMode off: {resp1:?}");

    // 2. Turning on Ghost Pro mode MUST automatically turn off Ghost Mode and Peer Exploration
    let resp2 = eng.post_settings(r#"{"ghostProMode":true}"#);
    assert!(resp2.ghost_pro_mode, "expected ghostProMode to be true: {resp2:?}");
    assert!(
        !resp2.ghost_mode,
        "expected ghostMode to be automatically turned OFF: {resp2:?}"
    );
    assert!(
        !resp2.peer_exploration,
        "expected peerExploration to be automatically turned OFF: {resp2:?}"
    );

    // 3. GET verify persistence
    let resp_get = eng.get_settings();
    assert!(
        resp_get.ghost_pro_mode,
        "GET /settings ghostProMode mismatch: {resp_get:?}"
    );
    assert!(
        !resp_get.ghost_mode,
        "GET /settings ghostMode mismatch: {resp_get:?}"
    );
    assert!(
        !resp_get.peer_exploration,
        "GET /settings peerExploration mismatch: {resp_get:?}"
    );

    // 4. Turning on Ghost Mode turns OFF Ghost Pro mode
    let resp3 = eng.post_settings(r#"{"ghostMode":true}"#);
    assert!(resp3.ghost_mode, "expected ghostMode ON: {resp3:?}");
    assert!(!resp3.ghost_pro_mode, "expected ghostProMode OFF: {resp3:?}");

    // 5. Turning on Ghost Pro mode again
    let resp4 = eng.post_settings(r#"{"ghostProMode":true}"#);
    assert!(resp4.ghost_pro_mode, "expected ghostProMode ON: {resp4:?}");
    assert!(!resp4.ghost_mode, "expected ghostMode OFF: {resp4:?}");

    // 6. Turning on Peer Exploration turns OFF Ghost Pro mode
    let resp5 = eng.post_settings(r#"{"peerExploration":true}"#);
    assert!(resp5.peer_exploration, "expected peerExploration ON: {resp5:?}");
    assert!(!resp5.ghost_pro_mode, "expected ghostProMode OFF: {resp5:?}");

    // 7. Turning off Ghost Pro mode directly
    let _ = eng.post_settings(r#"{"ghostProMode":true}"#);
    let resp_off = eng.post_settings(r#"{"ghostProMode":false}"#);
    assert!(!resp_off.ghost_pro_mode, "expected ghostProMode OFF: {resp_off:?}");
}

#[test]
fn test_ghost_pro_can_toggle_while_sampler_runs() {
    let (eng, _, _) = playback_fixture_mode(true);
    eng.post_settings(r#"{"maxPeers":55}"#);
    let server = eng.server.clone();
    let worker = std::thread::spawn(move || {
        let start = Instant::now() + Duration::from_secs(1000);
        for i in 0..200 { server.ghost_pro_mode_tick(start + Duration::from_secs(i)); }
    });
    for _ in 0..20 {
        assert!(eng.post_settings(r#"{"ghostProMode":true}"#).ghost_pro_mode);
        assert!(!eng.post_settings(r#"{"ghostProMode":false}"#).ghost_pro_mode);
    }
    worker.join().unwrap();
    assert_eq!(eng.get_settings().effective_dial_rate_per_sec, 30, "sampler overwrote disabled base");
}

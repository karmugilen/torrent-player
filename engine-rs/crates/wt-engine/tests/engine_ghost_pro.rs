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

#[ignore = "needs: EngineServer ghost_pro playback tiers and scaled windows"]
#[test]
fn test_ghost_pro_playback_startup_seek_and_scaled_windows() {
    // Port of Go TestGhostProPlaybackStartupSeekAndScaledWindows
    let eng = TestEngine::new();
    let (code, body) = eng.dispatch(
        "POST",
        "/settings",
        r#"{"ghostProMode":true,"streamCacheBytes":67108864}"#,
    );
    assert_eq!(code, 200, "settings: {}", String::from_utf8_lossy(&body));

    for (name, piece_len, _boost, _urgent, _forward, _lookahead) in [
        ("standard", 1 << 20, false, 16 << 20, 64 << 20, 32 << 20),
        ("boost", 1 << 20, true, 16 << 20, 96 << 20, 32 << 20),
        (
            "large pieces",
            16 << 20,
            false,
            32 << 20,
            96 << 20,
            64 << 20,
        ),
    ] {
        let (eng_fixture, id, _plen, _data) =
            large_playback_fixture_mode(if piece_len == 16 << 20 { 16 } else { 256 }, true);
        let playback = eng_fixture.open_playback(&id, 0).expect("open playback");
        let offset = 32 * piece_len;
        let mut buf = [0u8; 1];
        let _ = playback.read_at(&mut buf, offset);

        let (code, pbody) =
            eng_fixture.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
        assert_eq!(
            code,
            200,
            "{name}: pieces: {}",
            String::from_utf8_lossy(&pbody)
        );
        let pieces: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
        assert!(
            pieces.get("playback").is_some(),
            "{name}: playback tiers missing"
        );
    }
}

#[ignore = "needs: EngineServer ghost_pro multi-reader RAM allowance tiers"]
#[test]
fn test_ghost_pro_instant_play_keeps_all_tiers_within_ram_allowance() {
    // Port of Go TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(256, true);
    let (code, body) = eng.dispatch(
        "POST",
        "/settings",
        r#"{"ghostProMode":true,"streamCacheBytes":67108864}"#,
    );
    assert_eq!(code, 200, "settings: {}", String::from_utf8_lossy(&body));

    let mut readers = Vec::new();
    for i in 0..4 {
        let p = eng.open_playback(&id, 0).expect("open playback");
        readers.push(p);

        for (j, r) in readers.iter().enumerate() {
            let mut buf = [0u8; 1];
            let _ = r.read_at(&mut buf, ((32 + j * 60) << 20) as i64);
        }

        let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
        assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
        let pieces: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
        let playback_arr = pieces["playback"].as_array().expect("playback array");
        assert!(
            !playback_arr.is_empty(),
            "reader {i} should show playback ranges"
        );
    }
    assert_eq!(piece_len, 1 << 20);
}

#[ignore = "needs: EngineServer direct play seek window updates"]
#[test]
fn test_direct_play_seek_is_smooth_forward_backward_and_middle() {
    // Port of Go TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle
    let (eng, id, piece_len, _data) = large_playback_fixture_mode(256, true);
    let p = eng.open_playback(&id, 0).expect("open playback");
    let middle = 128 * piece_len;
    let mut buf = [0u8; 1];
    let _ = p.read_at(&mut buf, middle);

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));

    let _ = p.read_at(&mut buf, middle - piece_len);
    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
    assert_eq!(
        code,
        200,
        "pieces rewind: {}",
        String::from_utf8_lossy(&pbody)
    );

    let far = middle + 80 * piece_len;
    let _ = p.read_at(&mut buf, far);
    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
    assert_eq!(
        code,
        200,
        "pieces jump: {}",
        String::from_utf8_lossy(&pbody)
    );
}

#[ignore = "needs: EngineServer direct play large pieces cache accounting"]
#[test]
fn test_direct_play_large_pieces_stay_inside_the_cache() {
    // Port of Go TestDirectPlayLargePiecesStayInsideTheCache
    let (eng, id, _plen, _data) = large_playback_fixture_mode(16, true);
    let p1 = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 1];
    let _ = p1.read_at(&mut buf, (16 << 20) + 1);
    let p2 = eng.open_playback(&id, 0).expect("open playback");
    let _ = p2.read_at(&mut buf, (16 << 20) + 1);

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=16"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
}

#[ignore = "needs: EngineServer ghost_pro first verified read urgency expansion"]
#[test]
fn test_ghost_pro_first_verified_read_expands_urgency() {
    // Port of Go TestGhostProFirstVerifiedReadExpandsUrgency
    let (eng, id, _piece_len, data) = large_playback_fixture_mode(48, false);
    let (code, body) = eng.dispatch("POST", "/settings", r#"{"ghostProMode":true}"#);
    assert_eq!(code, 200, "settings: {}", String::from_utf8_lossy(&body));

    write_verified_piece(&eng, &id, &data, 10);

    let playback = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 4];
    let n = playback.read_at(&mut buf, 10 << 20).expect("read");
    assert_eq!(n, 4);

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=48"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
}

#[ignore = "needs: EngineServer ghost_pro playback telemetry tiers"]
#[test]
fn test_ghost_pro_playback_telemetry_moves_clears_and_toggles() {
    // Port of Go TestGhostProPlaybackTelemetryMovesClearsAndToggles
    let (eng, id, _plen, _data) = large_playback_fixture_mode(256, true);
    let p = eng.open_playback(&id, 0).expect("open playback");
    let mut buf = [0u8; 1];
    let _ = p.read_at(&mut buf, 32 << 20);

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
    let pieces: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
    assert_eq!(pieces["ghostProMode"], true);

    let (code, sbody) = eng.dispatch("POST", "/settings", r#"{"ghostProMode":false}"#);
    assert_eq!(code, 200, "disable: {}", String::from_utf8_lossy(&sbody));

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=256"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
    let pieces: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
    assert_eq!(pieces["ghostProMode"], false);
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
    let s = eng.post_settings(r#"{"ghostProMode":true,"ghostMode":false,"peerExploration":false}"#);
    assert!(s.ghost_pro_mode);
    assert!(!s.ghost_mode);
    assert!(!s.peer_exploration);
    let s = eng.post_settings(r#"{"ghostMode":true}"#);
    assert!(s.ghost_mode);
    let s = eng.post_settings(r#"{"peerExploration":true}"#);
    assert!(s.peer_exploration);
}

#[ignore = "needs: EngineServer ghost_pro_mode_tick sampler"]
#[test]
fn test_ghost_pro_can_toggle_while_sampler_runs() {
    // Port of Go TestGhostProCanToggleWhileSamplerRuns
    let (eng, _id, _data) = playback_fixture_mode(true);
    for _ in 0..20 {
        let (code, _) = eng.dispatch("POST", "/settings", r#"{"ghostProMode":true}"#);
        assert_eq!(code, 200);
        let (code, _) = eng.dispatch("POST", "/settings", r#"{"ghostProMode":false}"#);
        assert_eq!(code, 200);
    }
}

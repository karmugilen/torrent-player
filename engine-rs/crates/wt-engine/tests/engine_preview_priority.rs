//! Integration tests ported from `engine-go/preview_priority_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
use support::TestEngine;

#[ignore = "needs: preview not starved by startup hints in multi-episode swarm"]
#[test]
fn test_preview_of_later_episode_is_not_starved_by_startup_hints() {
    // Port of Go `TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints`.
    let eng = TestEngine::new();
    let episode = 24 << 20i64;
    let (torrent, _, _) = support::build_multi_file_torrent(
        "Season",
        64 << 10,
        &[
            ("e1.mkv", episode),
            ("e2.mkv", episode),
            ("e3.mkv", episode),
            ("e4.mkv", episode),
        ],
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();

    let reader = eng.server.open_preview(id, 3).expect("open_preview");
    let mut got = vec![0u8; 256 << 10];
    let _ = reader.read_at(&mut got, episode / 2);
    reader.close();
}

#[ignore = "needs: preview wakes resting direct play"]
#[test]
fn test_preview_wakes_resting_direct_play() {
    // Port of Go `TestPreviewWakesRestingDirectPlay`.
    let (eng, id, _piece_len, _data) = support::large_playback_fixture_mode(40, true);
    let _ = eng.post_settings(r#"{"idleDropMinutes":5}"#);
    let player = eng.server.open_playback(&id, 0).expect("open_playback");
    let mut buf = [0u8; 64];
    let _ = player.read_at(&mut buf, 0);
    player.close();

    let reader = eng.server.open_preview(&id, 0).expect("open_preview");
    let mut got = vec![0u8; 256 << 10];
    let _ = reader.read_at(&mut got, 20 << 20);
    reader.close();
}

#[ignore = "needs: preview read and close deadlock protection with record lock"]
#[test]
fn test_reads_waking_resting_direct_play_do_not_deadlock_with_record_lock() {
    // Port of Go `TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock`.
    let (eng, id, _piece_len, _data) = support::large_playback_fixture_mode(40, true);
    let _ = eng.post_settings(r#"{"idleDropMinutes":5}"#);
    let preview = eng.server.open_preview(&id, 0).expect("open_preview");
    let player = eng.server.open_playback(&id, 0).expect("open_playback");
    preview.close();
    player.close();
}

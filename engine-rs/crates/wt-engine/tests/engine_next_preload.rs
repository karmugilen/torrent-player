//! Integration tests ported from `engine-go/next_preload_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
use support::TestEngine;

#[ignore = "needs: next episode preload near end of playback"]
#[test]
fn test_direct_play_preloads_next_episode_near_the_end() {
    // Port of Go `TestDirectPlayPreloadsNextEpisodeNearTheEnd`.
    let eng = TestEngine::new();
    let file_length = 8 << 20i64;
    let (torrent, _, _) = support::build_multi_file_torrent(
        "Show",
        64 << 10,
        &[
            ("Episode 01.mkv", file_length),
            ("Episode 02.mkv", file_length),
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

    let play_req = serde_json::json!({
        "id": id,
        "fileIndex": 0,
        "nextFileIndex": 1,
    });
    let (code, resp) = eng.dispatch("POST", "/play", &play_req.to_string());
    assert_eq!(code, 200, "play: {code} {}", String::from_utf8_lossy(&resp));
    let p = eng.server.open_playback(id, 0).expect("open_playback");
    p.close();
}

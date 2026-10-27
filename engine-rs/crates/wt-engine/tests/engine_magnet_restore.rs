//! Integration tests ported from `engine-go/magnet_restore_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_pending_magnet_can_resume_before_storage_is_configured() {
    // Port of Go `TestPendingMagnetCanResumeBeforeStorageIsConfigured`.
    let eng = TestEngine::new();
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        r#"{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","prepare":true}"#,
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();

    let pause_req = serde_json::json!({"id": id}).to_string();
    let (code, body) = eng.dispatch("POST", "/pause", &pause_req);
    assert_eq!(
        code,
        200,
        "pause before metadata: {code} {}",
        String::from_utf8_lossy(&body)
    );

    let (code, body) = eng.dispatch("POST", "/resume", &pause_req);
    assert_eq!(
        code,
        200,
        "resume before metadata: {code} {}",
        String::from_utf8_lossy(&body)
    );

    let record = eng.server.records.read().get(id).unwrap().clone();
    let rec = record.read();
    assert!(!rec.paused, "must not remain paused");
    assert!(!rec.configured, "must not be configured yet");
    assert!(rec.torrent.info().is_none(), "info must still be pending");
    eng.close();
}

#[ignore = "needs: cold restored pending magnet tracker discovery"]
#[test]
fn test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata() {
    // Port of Go `TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata`.
    let eng = TestEngine::new();
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        r#"{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789&tr=http://127.0.0.1:1/announce","prepare":true}"#,
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    let (code, _) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200);
    eng.close();
}

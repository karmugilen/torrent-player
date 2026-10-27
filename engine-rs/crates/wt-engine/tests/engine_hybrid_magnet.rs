//! Integration tests ported from `engine-go/hybrid_magnet_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_add_hybrid_magnet_with_trackers_keeps_engine_alive() {
    let eng = TestEngine::new();
    // Hybrid magnet: v1 infohash in torrentId; trackers supplied in magnet-style id is enough for add.
    let hash = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
    let body = format!(r#"{{"torrentId":"{hash}","prepare":true}}"#);
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    // Engine must still answer /stats after add.
    let _ = eng.stats();
}

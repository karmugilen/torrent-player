//! Integration tests ported from `engine-go/hybrid_magnet_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_add_hybrid_magnet_with_trackers_keeps_engine_alive() {
    let eng = TestEngine::new();
    let magnet = "magnet:?xt=urn:btih:643bf2b0362ff2548c0d34c56762e74e2302f88f&xt=urn:btmh:1220deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef&dn=hybrid&tr=http%3A%2F%2F127.0.0.1%3A1%2Fannounce&tr=udp%3A%2F%2F127.0.0.1%3A1%2Fannounce";
    let request = serde_json::json!({"torrentId":magnet,"prepare":true});
    let (code, raw) = eng.dispatch("POST", "/add", &request.to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

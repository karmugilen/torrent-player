//! Integration tests ported from `engine-go/network_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_network_change_ignores_stale_and_duplicate_generations() {
    let eng = TestEngine::new();
    let post = |generation, connected| {
        let (code, raw) = eng.dispatch("POST", "/network", &serde_json::json!({"generation":generation,"connected":connected}).to_string());
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    };
    post(1, true);
    assert!(eng.server.network.known());
    assert_eq!(eng.server.network.generation(), 1);
    let episode = eng.server.network.episode();
    post(1, false);
    assert!(eng.server.network.known());
    assert_eq!(eng.server.network.generation(), 1, "duplicate mutated generation");
    assert_eq!(eng.server.network.episode(), episode);
    post(0, false);
    assert_eq!(eng.server.network.generation(), 1, "stale event mutated generation");
    post(2, true);
    assert_eq!(eng.server.network.generation(), 2);

}

#[test]
fn test_network_change_rejects_invalid_body() {
    let eng = TestEngine::new();
    let (code, raw) = eng.dispatch("POST", "/network", "not-json");
    assert_eq!(code, 400, "{}", String::from_utf8_lossy(&raw));
    let (code, raw) = eng.dispatch("POST", "/network", r#"{"generation":1}"#);
    assert_eq!(code, 400, "missing connected: {}", String::from_utf8_lossy(&raw));
    let payload: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(!payload["error"].as_str().unwrap().is_empty());
}

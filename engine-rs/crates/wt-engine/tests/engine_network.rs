//! Integration tests ported from `engine-go/network_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_network_change_ignores_stale_and_duplicate_generations() {
    let eng = TestEngine::new();
    let (code, raw) = eng.dispatch("POST", "/network", r#"{"generation":2,"connected":true}"#);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let (code, raw) = eng.dispatch("POST", "/network", r#"{"generation":1,"connected":true}"#);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let (code, raw) = eng.dispatch("POST", "/network", r#"{"generation":2,"connected":false}"#);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[test]
fn test_network_change_rejects_invalid_body() {
    let eng = TestEngine::new();
    let (code, raw) = eng.dispatch("POST", "/network", "not-json");
    assert_eq!(code, 400, "{}", String::from_utf8_lossy(&raw));
}

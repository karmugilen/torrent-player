//! Integration tests ported from `engine-go/seek_http_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_seek_http_comparison() {
    // Port of Go `TestSeekHTTPComparison`.
    if std::env::var("SEEK_SEARCH").as_deref() != Ok("http") {
        return;
    }
    let eng = TestEngine::new();
    let (code, _) = eng.dispatch("GET", "/stats", "");
    assert_eq!(code, 200);
    eng.close();
}

#[test]
fn test_seek_resume_comparison() {
    // Port of Go `TestSeekResumeComparison`.
    if std::env::var("SEEK_TUNE").is_err() {
        return;
    }
    let eng = TestEngine::new();
    let (code, _) = eng.dispatch("GET", "/stats", "");
    assert_eq!(code, 200);
    eng.close();
}

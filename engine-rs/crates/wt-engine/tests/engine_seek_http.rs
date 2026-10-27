//! Integration tests ported from `engine-go/seek_http_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_seek_http_comparison() {
    // Port of Go `TestSeekHTTPComparison`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSeekHTTPComparison");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_seek_resume_comparison() {
    // Port of Go `TestSeekResumeComparison`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSeekResumeComparison");
}

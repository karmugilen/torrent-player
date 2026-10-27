//! Integration tests ported from `engine-go/playback_resume_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_close_reopen_preserves_resume_across_metadata_probes() {
    // Port of Go `TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_open_playback_reuses_validated_resume_window() {
    // Port of Go `TestOpenPlaybackReusesValidatedResumeWindow`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestOpenPlaybackReusesValidatedResumeWindow");
}

//! Integration tests ported from `engine-go/live_preview_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_live_preview_state_follows_verified_playback_position() {
    // Port of Go `TestLivePreviewStateFollowsVerifiedPlaybackPosition`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestLivePreviewStateFollowsVerifiedPlaybackPosition");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_handle_survives_download_completion() {
    // Port of Go `TestPlaybackHandleSurvivesDownloadCompletion`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackHandleSurvivesDownloadCompletion");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playing_preview_does_not_demand_missing_pieces() {
    // Port of Go `TestPlayingPreviewDoesNotDemandMissingPieces`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlayingPreviewDoesNotDemandMissingPieces");
}

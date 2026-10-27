//! Integration tests ported from `engine-go/preview_priority_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_of_later_episode_is_not_starved_by_startup_hints() {
    // Port of Go `TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_wakes_resting_direct_play() {
    // Port of Go `TestPreviewWakesRestingDirectPlay`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewWakesRestingDirectPlay");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_reads_waking_resting_direct_play_do_not_deadlock_with_record_lock() {
    // Port of Go `TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock");
}

//! Integration tests ported from `engine-go/direct_playback_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_direct_play_local_peer_seeks_beyond_ram_budget() {
    // Port of Go `TestDirectPlayLocalPeerSeeksBeyondRAMBudget`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlayLocalPeerSeeksBeyondRAMBudget");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_idle_reader_parks_and_resumes_the_same_descriptor() {
    // Port of Go `TestIdleReaderParksAndResumesTheSameDescriptor`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestIdleReaderParksAndResumesTheSameDescriptor");
}

#[ignore = "waiting for live peer connections"]
#[test]
fn test_stalled_read_retries_without_returning_zeros_or_dropping_peers() {
    // Port of Go `TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_direct_play_cancels_wait_on_seek_and_close() {
    // Port of Go `TestDirectPlayCancelsWaitOnSeekAndClose`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlayCancelsWaitOnSeekAndClose");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_claude_seek_with_slow_and_fast_seed() {
    // Port of Go `TestClaudeSeekWithSlowAndFastSeed`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestClaudeSeekWithSlowAndFastSeed");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_claude_seek_setting_toggles_stealing_and_resets_stats() {
    // Port of Go `TestClaudeSeekSettingTogglesStealingAndResetsStats`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestClaudeSeekSettingTogglesStealingAndResetsStats");
}

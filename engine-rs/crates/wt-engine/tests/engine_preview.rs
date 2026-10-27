//! Integration tests ported from `engine-go/preview_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_open_preview_reads_verified_bytes_without_focus_change() {
    // Port of Go `TestOpenPreviewReadsVerifiedBytesWithoutFocusChange`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestOpenPreviewReadsVerifiedBytesWithoutFocusChange");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_rejects_second_concurrent_job() {
    // Port of Go `TestPreviewRejectsSecondConcurrentJob`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewRejectsSecondConcurrentJob");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_temp_for_ready_draft_without_full_file() {
    // Port of Go `TestPreviewTempForReadyDraftWithoutFullFile`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewTempForReadyDraftWithoutFullFile");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_close_is_idempotent_and_cancels_read() {
    // Port of Go `TestPreviewCloseIsIdempotentAndCancelsRead`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewCloseIsIdempotentAndCancelsRead");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_pause_releases_preview_demand() {
    // Port of Go `TestPauseReleasesPreviewDemand`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPauseReleasesPreviewDemand");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_budget_enforced_across_seeks() {
    // Port of Go `TestPreviewBudgetEnforcedAcrossSeeks`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewBudgetEnforcedAcrossSeeks");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_session_deadline_blocks_unlimited_reads() {
    // Port of Go `TestPreviewSessionDeadlineBlocksUnlimitedReads`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewSessionDeadlineBlocksUnlimitedReads");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_close_joins_blocked_read() {
    // Port of Go `TestPreviewCloseJoinsBlockedRead`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewCloseJoinsBlockedRead");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_concurrent_open_atomic() {
    // Port of Go `TestPreviewConcurrentOpenAtomic`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewConcurrentOpenAtomic");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_pause_other_torrent_leaves_preview() {
    // Port of Go `TestPauseOtherTorrentLeavesPreview`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPauseOtherTorrentLeavesPreview");
}

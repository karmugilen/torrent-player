//! Integration tests ported from `engine-go/playback_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_reads_verified_bytes_without_waiting() {
    // Port of Go `TestPlaybackReadsVerifiedBytesWithoutWaiting`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackReadsVerifiedBytesWithoutWaiting");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_waits_for_verification_then_supports_seeking() {
    // Port of Go `TestPlaybackWaitsForVerificationThenSupportsSeeking`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackWaitsForVerificationThenSupportsSeeking");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_window_follows_seek_and_demotes_old_now() {
    // Port of Go `TestPlaybackWindowFollowsSeekAndDemotesOldNow`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackWindowFollowsSeekAndDemotesOldNow");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_scrub_has_immediate_full_readahead() {
    // Port of Go `TestPlaybackScrubHasImmediateFullReadahead`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackScrubHasImmediateFullReadahead");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_handles_share_priority_ownership() {
    // Port of Go `TestPlaybackHandlesSharePriorityOwnership`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackHandlesSharePriorityOwnership");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_seek_yields_startup_hints_to_target() {
    // Port of Go `TestPlaybackSeekYieldsStartupHintsToTarget`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackSeekYieldsStartupHintsToTarget");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_no_seek_hotspots_retained() {
    // Port of Go `TestPlaybackNoSeekHotspotsRetained`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackNoSeekHotspotsRetained");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_debounce_timer_quiet_expiration() {
    // Port of Go `TestPlaybackDebounceTimerQuietExpiration`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackDebounceTimerQuietExpiration");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_gated_open_phase_dual_now_and_moov_probe() {
    // Port of Go `TestPlaybackGatedOpenPhaseDualNowAndMoovProbe`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackGatedOpenPhaseDualNowAndMoovProbe");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_open_phase_exit_on_body_read() {
    // Port of Go `TestPlaybackOpenPhaseExitOnBodyRead`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackOpenPhaseExitOnBodyRead");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_file_piece_range_and_overlap_helpers() {
    // Port of Go `TestFilePieceRangeAndOverlapHelpers`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestFilePieceRangeAndOverlapHelpers");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_close_cancels_blocked_read() {
    // Port of Go `TestPlaybackCloseCancelsBlockedRead`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackCloseCancelsBlockedRead");
}

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_completed_and_streaming_http_ranges() {
    // Port of Go `TestCompletedAndStreamingHTTPRanges`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestCompletedAndStreamingHTTPRanges");
}

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_http_disconnect_cancels_missing_range_and_another_request_still_works() {
    // Port of Go `TestHTTPDisconnectCancelsMissingRangeAndAnotherRequestStillWorks`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestHTTPDisconnectCancelsMissingRangeAndAnotherRequestStillWorks");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_torrent_status_done_uses_verified_not_only_live_progress() {
    // Port of Go `TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_rejects_unselected_files_and_unverified_completion() {
    // Port of Go `TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion");
}

#[ignore = "waiting for live peer connections"]
#[test]
fn test_local_peer_download_produces_playable_media() {
    // Port of Go `TestLocalPeerDownloadProducesPlayableMedia`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestLocalPeerDownloadProducesPlayableMedia");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_boost_request_decodes() {
    // Port of Go `TestPlaybackBoostRequestDecodes`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackBoostRequestDecodes");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playback_boost_startup_window() {
    // Port of Go `TestPlaybackBoostStartupWindow`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlaybackBoostStartupWindow");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_http_playback_writes_a_seek_log_line() {
    // Port of Go `TestHTTPPlaybackWritesASeekLogLine`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestHTTPPlaybackWritesASeekLogLine");
}

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_unverified_reads_skip_the_start_and_end_of_the_file() {
    // Port of Go `TestUnverifiedReadsSkipTheStartAndEndOfTheFile`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestUnverifiedReadsSkipTheStartAndEndOfTheFile");
}

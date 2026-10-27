//! Integration tests ported from `engine-go/playback_grid_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for rangeStart helper / HTTP Range parsing export"]
#[test]
fn test_range_start_parses_single_ranges() {
    // Port of Go `TestRangeStartParsesSingleRanges`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestRangeStartParsesSingleRanges");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_seek_range_opens_the_window_at_the_range() {
    // Port of Go `TestSeekRangeOpensTheWindowAtTheRange`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSeekRangeOpensTheWindowAtTheRange");
}

#[ignore = "waiting for GET /pieces HTTP + playback tiers"]
#[test]
fn test_pieces_reports_playback_tiers_playhead_and_detail() {
    // Port of Go `TestPiecesReportsPlaybackTiersPlayheadAndDetail`.
    // Pure tier/playhead builders live in wt-policy; this covers the HTTP route.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPiecesReportsPlaybackTiersPlayheadAndDetail");
}

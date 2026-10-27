//! Integration tests ported from `engine-go/playback_grid_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

use engine::stream_http::range_start;

#[test]
fn test_range_start_parses_single_ranges() {
    // Port of Go `TestRangeStartParsesSingleRanges`.
    let cases: &[(&str, i64)] = &[
        ("bytes=0-", 0),
        ("bytes=1048576-", 1 << 20),
        ("bytes=10-20", 10),
        (" bytes=7- ", 7),
        ("", -1),
        ("bytes=-500", -1),
        ("bytes=1-2,5-6", -1),
        ("items=1-", -1),
        ("bytes=x-", -1),
    ];
    for &(header, want) in cases {
        assert_eq!(
            range_start(header),
            want,
            "range_start({header:?}) = {}, want {want}",
            range_start(header)
        );
    }
    let _eng = TestEngine::new();
}

#[ignore = "waiting for EngineServer PlaybackHost wiring + peers"]
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

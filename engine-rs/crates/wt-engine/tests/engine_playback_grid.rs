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

#[ignore = "needs: open_playback_at with seek offset opening window at range"]
#[test]
fn test_seek_range_opens_the_window_at_the_range() {
    // Port of Go `TestSeekRangeOpensTheWindowAtTheRange`.
    let (eng, id, _piece_len, _data) = support::large_playback_fixture_mode(256, false);
    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    let mut buf = [0u8; 1];
    let _ = p.read_at(&mut buf, 100 << 20);
    p.close();
}

#[ignore = "needs: GET /pieces reporting playback tiers, playhead and detail"]
#[test]
fn test_pieces_reports_playback_tiers_playhead_and_detail() {
    // Port of Go `TestPiecesReportsPlaybackTiersPlayheadAndDetail`.
    let (eng, id, _piece_len, _data) = support::large_playback_fixture_mode(256, false);
    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    let mut buf = [0u8; 1];
    let _ = p.read_at(&mut buf, 40 << 20);
    let (code, raw) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(
        code,
        200,
        "pieces: {code} {}",
        String::from_utf8_lossy(&raw)
    );
    let resp: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(resp.get("playheads").is_some());
    p.close();
}

//! Integration tests ported from `engine-go/playback_active_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_only_the_latest_seek_position_is_fetched() {
    // Port of Go `TestOnlyTheLatestSeekPositionIsFetched`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestOnlyTheLatestSeekPositionIsFetched");
}

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_blocked_read_at_an_earlier_spot_stops_and_stays_stopped() {
    // Port of Go `TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped");
}

//! Integration tests ported from `engine-go/next_preload_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_direct_play_preloads_next_episode_near_the_end() {
    // Port of Go `TestDirectPlayPreloadsNextEpisodeNearTheEnd`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlayPreloadsNextEpisodeNearTheEnd");
}

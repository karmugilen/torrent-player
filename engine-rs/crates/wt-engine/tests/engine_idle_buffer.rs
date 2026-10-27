//! Integration tests ported from `engine-go/idle_buffer_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_playing_reader_keeps_window_and_peers_while_idle() {
    // Port of Go `TestPlayingReaderKeepsWindowAndPeersWhileIdle`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPlayingReaderKeepsWindowAndPeersWhileIdle");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_former_playhead_parks_once_it_is_no_longer_playing() {
    // Port of Go `TestFormerPlayheadParksOnceItIsNoLongerPlaying`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestFormerPlayheadParksOnceItIsNoLongerPlaying");
}

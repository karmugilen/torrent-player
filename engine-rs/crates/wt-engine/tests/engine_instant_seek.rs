//! Integration tests ported from `engine-go/instant_seek_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_seek_serves_delivered_blocks_before_the_piece_is_verified() {
    // Port of Go `TestSeekServesDeliveredBlocksBeforeThePieceIsVerified`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSeekServesDeliveredBlocksBeforeThePieceIsVerified");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_reopening_the_same_file_does_not_replan_metadata() {
    // Port of Go `TestReopeningTheSameFileDoesNotReplanMetadata`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestReopeningTheSameFileDoesNotReplanMetadata");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_reinjecting_resume_bytes_does_not_leak_cache_accounting() {
    // Port of Go `TestReinjectingResumeBytesDoesNotLeakCacheAccounting`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestReinjectingResumeBytesDoesNotLeakCacheAccounting");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_resume_window_saves_once_seeking_settles_and_reopens_from_ram() {
    // Port of Go `TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM");
}

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_http_read_hands_over_the_first_block_at_once() {
    // Port of Go `TestHTTPReadHandsOverTheFirstBlockAtOnce`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestHTTPReadHandsOverTheFirstBlockAtOnce");
}

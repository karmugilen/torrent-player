//! Integration tests ported from `engine-go/metadata_cache_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_direct_play_saves_every_episodes_metadata_once() {
    // Port of Go `TestDirectPlaySavesEveryEpisodesMetadataOnce`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlaySavesEveryEpisodesMetadataOnce");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_direct_play_fetches_every_episodes_metadata_in_background() {
    // Port of Go `TestDirectPlayFetchesEveryEpisodesMetadataInBackground`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlayFetchesEveryEpisodesMetadataInBackground");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_validated_saved_metadata_replaces_partial_ram_piece() {
    // Port of Go `TestValidatedSavedMetadataReplacesPartialRAMPiece`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestValidatedSavedMetadataReplacesPartialRAMPiece");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_saved_metadata_commit_drops_partial_ram() {
    // Port of Go `TestSavedMetadataCommitDropsPartialRAM`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSavedMetadataCommitDropsPartialRAM");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_saved_metadata_commit_clears_evicted_flag() {
    // Port of Go `TestSavedMetadataCommitClearsEvictedFlag`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSavedMetadataCommitClearsEvictedFlag");
}

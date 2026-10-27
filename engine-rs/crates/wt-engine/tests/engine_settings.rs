//! Integration tests ported from `engine-go/settings_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for cache resize / torrent apply path"]
#[test]
fn test_settings_apply_to_existing_and_future_torrents() {
    // Port of Go `TestSettingsApplyToExistingAndFutureTorrents`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSettingsApplyToExistingAndFutureTorrents");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_cache_shrink_preserves_dirty_download_bytes() {
    // Port of Go `TestCacheShrinkPreservesDirtyDownloadBytes`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestCacheShrinkPreservesDirtyDownloadBytes");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_cache_resize_keeps_direct_play_readable_and_preview_bounded() {
    // Port of Go `TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_small_ram_setting_keeps_four_direct_play_pieces() {
    // Port of Go `TestSmallRAMSettingKeepsFourDirectPlayPieces`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSmallRAMSettingKeepsFourDirectPlayPieces");
}

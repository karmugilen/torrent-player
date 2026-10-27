//! Integration tests ported from `engine-go/delivery_regression_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_delivery_preview_can_reuse_verified_tail() {
    // Port of Go `TestDeliveryPreviewCanReuseVerifiedTail`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDeliveryPreviewCanReuseVerifiedTail");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_delivery_warm_cache_preserves_explicit_play_from_start() {
    // Port of Go `TestDeliveryWarmCachePreservesExplicitPlayFromStart`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDeliveryWarmCachePreservesExplicitPlayFromStart");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_delivery_concurrent_cache_clears() {
    // Port of Go `TestDeliveryConcurrentCacheClears`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDeliveryConcurrentCacheClears");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_delivery_expired_preview_releases_slot() {
    // Port of Go `TestDeliveryExpiredPreviewReleasesSlot`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDeliveryExpiredPreviewReleasesSlot");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_release_invalidates_temporary_completion() {
    // Port of Go `TestPreviewReleaseInvalidatesTemporaryCompletion`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewReleaseInvalidatesTemporaryCompletion");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_preview_honors_payload_gate() {
    // Port of Go `TestPreviewHonorsPayloadGate`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPreviewHonorsPayloadGate");
}

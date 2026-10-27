//! Integration tests ported from `engine-go/trackers_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_public_tracker_parsing() {
    // Port of Go `TestPublicTrackerParsing`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPublicTrackerParsing");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_cache_conditional_refresh_and_failure() {
    // Port of Go `TestTrackerCacheConditionalRefreshAndFailure`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerCacheConditionalRefreshAndFailure");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_cache_corruption_and_fallback() {
    // Port of Go `TestTrackerCacheCorruptionAndFallback`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerCacheCorruptionAndFallback");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_fetch_is_async_and_cancelled_on_close() {
    // Port of Go `TestTrackerFetchIsAsyncAndCancelledOnClose`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerFetchIsAsyncAndCancelledOnClose");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_supplement_public_private_and_magnet() {
    // Port of Go `TestTrackerSupplementPublicPrivateAndMagnet`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerSupplementPublicPrivateAndMagnet");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_refresh_is_bounded_for_existing_torrents() {
    // Port of Go `TestTrackerRefreshIsBoundedForExistingTorrents`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerRefreshIsBoundedForExistingTorrents");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_discovered_peer_survives_refresh() {
    // Port of Go `TestTrackerDiscoveredPeerSurvivesRefresh`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerDiscoveredPeerSurvivesRefresh");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_managed_trackers_do_not_accumulate_across_restore() {
    // Port of Go `TestManagedTrackersDoNotAccumulateAcrossRestore`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestManagedTrackersDoNotAccumulateAcrossRestore");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_add_magnet_ignores_dht_label_and_keeps_tracker_ur_ls() {
    // Port of Go `TestAddMagnetIgnoresDHTLabelAndKeepsTrackerURLs`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestAddMagnetIgnoresDHTLabelAndKeepsTrackerURLs");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_supplied_tracker_validation_preserves_private_passkeys_and_tiers() {
    // Port of Go `TestSuppliedTrackerValidationPreservesPrivatePasskeysAndTiers`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSuppliedTrackerValidationPreservesPrivatePasskeysAndTiers");
}

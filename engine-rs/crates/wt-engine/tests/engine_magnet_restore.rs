//! Integration tests ported from `engine-go/magnet_restore_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_pending_magnet_can_resume_before_storage_is_configured() {
    // Port of Go `TestPendingMagnetCanResumeBeforeStorageIsConfigured`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPendingMagnetCanResumeBeforeStorageIsConfigured");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata() {
    // Port of Go `TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata");
}

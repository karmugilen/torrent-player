//! Integration tests ported from `engine-go/telemetry_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_status_fingerprint_ignores_playback_priority_churn() {
    // Port of Go `TestStatusFingerprintIgnoresPlaybackPriorityChurn`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestStatusFingerprintIgnoresPlaybackPriorityChurn");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_status_fingerprint_wakes_on_verification_and_rates() {
    // Port of Go `TestStatusFingerprintWakesOnVerificationAndRates`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestStatusFingerprintWakesOnVerificationAndRates");
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_verified_file_bytes_cache_matches_walk() {
    // Port of Go `TestVerifiedFileBytesCacheMatchesWalk`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestVerifiedFileBytesCacheMatchesWalk");
}

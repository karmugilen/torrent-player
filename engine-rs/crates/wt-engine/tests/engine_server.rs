//! Integration tests ported from `engine-go/server_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for live peer connections"]
#[test]
fn test_status_reports_connections_not_discovered_candidates() {
    // Port of Go `TestStatusReportsConnectionsNotDiscoveredCandidates`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestStatusReportsConnectionsNotDiscoveredCandidates");
}

#[test]
fn test_add_magnet_before_metadata_returns_record() {
    let eng = TestEngine::new();
    let hash = "0123456789012345678901234567890123456789";
    let body = format!(r#"{{"torrentId":"{hash}","prepare":true}}"#);
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    assert!(!id.is_empty());
    let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_pause_is_reported_and_resume_restores_selection() {
    // Port of Go `TestPauseIsReportedAndResumeRestoresSelection`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPauseIsReportedAndResumeRestoresSelection");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_payload_transfer_setting_gates_bytes_but_preserves_manual_pause() {
    // Port of Go `TestPayloadTransferSettingGatesBytesButPreservesManualPause`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPayloadTransferSettingGatesBytesButPreservesManualPause");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_payload_gate_keeps_pending_magnet_discoverable() {
    // Port of Go `TestPayloadGateKeepsPendingMagnetDiscoverable`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPayloadGateKeepsPendingMagnetDiscoverable");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_payload_gate_rejects_incomplete_playback_clearly() {
    // Port of Go `TestPayloadGateRejectsIncompletePlaybackClearly`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPayloadGateRejectsIncompletePlaybackClearly");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_piece_telemetry_reports_selected_pieces() {
    // Port of Go `TestPieceTelemetryReportsSelectedPieces`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPieceTelemetryReportsSelectedPieces");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_accumulate_bucket_piece_selected_mixed_counts() {
    // Port of Go `TestAccumulateBucketPieceSelectedMixedCounts`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestAccumulateBucketPieceSelectedMixedCounts");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_piece_telemetry_selected_verified_ignores_excluded() {
    // Port of Go `TestPieceTelemetrySelectedVerifiedIgnoresExcluded`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPieceTelemetrySelectedVerifiedIgnoresExcluded");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_piece_telemetry_shared_boundary_counts_as_selected() {
    // Port of Go `TestPieceTelemetrySharedBoundaryCountsAsSelected`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPieceTelemetrySharedBoundaryCountsAsSelected");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_piece_telemetry_inclusive_ranges_and_max_buckets() {
    // Port of Go `TestPieceTelemetryInclusiveRangesAndMaxBuckets`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPieceTelemetryInclusiveRangesAndMaxBuckets");
}

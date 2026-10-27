//! Integration tests ported from `engine-go/server_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
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
    let body = format!(r#"{{"torrentId":"magnet:?xt=urn:btih:{hash}","prepare":true}}"#);
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    assert!(!id.is_empty());
    let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[test]
fn test_pause_is_reported_and_resume_restores_selection() {
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("tiny", 4096, 2);
    let body = serde_json::json!({"prepare": true, "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent)}).to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();
    let record = eng.server.records.read().get(id).unwrap().clone();
    {
        let mut rec = record.write();
        rec.configured = true;
        rec.selected = vec![0];
        rec.torrent.files()[0].download();
    }
    let request = serde_json::json!({"id": id}).to_string();
    assert_eq!(eng.dispatch("POST", "/pause", &request).0, 200);
    assert!(record.read().paused);
    let (status, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(status, 200);
    let view: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(view["paused"], true);
    assert_eq!(eng.dispatch("POST", "/resume", &request).0, 200);
    let rec = record.read();
    assert!(!rec.paused);
    let file = &rec.torrent.files()[0];
    assert_eq!(
        file.torrent.pieces.read()[0].base_priority,
        wt_core::PiecePriority::Normal
    );
}

#[test]
fn test_payload_transfer_setting_gates_bytes_but_preserves_manual_pause() {
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("tiny", 4096, 2);
    let body = serde_json::json!({"prepare": true, "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent)}).to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();
    let record = eng.server.records.read().get(id).unwrap().clone();
    let file = {
        let mut rec = record.write();
        rec.configured = true;
        rec.selected = vec![0];
        let file = rec.torrent.files()[0].clone();
        file.download();
        file
    };
    let priority = || file.torrent.pieces.read()[0].base_priority;
    assert_eq!(priority(), wt_core::PiecePriority::Normal);
    assert!(
        !eng.post_settings(r#"{"payloadTransfersAllowed":false}"#)
            .payload_transfers_allowed
    );
    assert!(!record.read().paused);
    assert_eq!(priority(), wt_core::PiecePriority::None);
    let req = serde_json::json!({"id": id}).to_string();
    assert_eq!(eng.dispatch("POST", "/pause", &req).0, 200);
    assert!(
        eng.post_settings(r#"{"payloadTransfersAllowed":true}"#)
            .payload_transfers_allowed
    );
    assert!(record.read().paused);
    assert_eq!(priority(), wt_core::PiecePriority::None);
    assert_eq!(eng.dispatch("POST", "/resume", &req).0, 200);
    assert!(!record.read().paused);
    assert_eq!(priority(), wt_core::PiecePriority::Normal);
    assert!(eng.get_settings().payload_transfers_allowed);
}

#[test]
fn test_payload_gate_keeps_pending_magnet_discoverable() {
    let eng = TestEngine::new();
    let (status, raw) = eng.dispatch("POST", "/add", r#"{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","prepare":true}"#);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    let record = eng.server.records.read().get(id).unwrap().clone();
    assert!(record.read().torrent.info().is_none());
    assert!(
        !eng.post_settings(r#"{"payloadTransfersAllowed":false}"#)
            .payload_transfers_allowed
    );
    let rec = record.read();
    assert!(!rec.paused);
    assert!(rec.torrent.info().is_none());
    assert_eq!(eng.server.client.torrents().len(), 1);
}

#[ignore = "waiting for play to reject incomplete files when payload transfers are disabled"]
#[test]
fn test_payload_gate_rejects_incomplete_playback_clearly() {
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("tiny", 4096, 2);
    let body = serde_json::json!({"prepare": true, "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent)}).to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();
    let record = eng.server.records.read().get(id).unwrap().clone();
    {
        let mut rec = record.write();
        rec.configured = true;
        rec.selected = vec![0];
    }
    assert!(
        !eng.post_settings(r#"{"payloadTransfersAllowed":false}"#)
            .payload_transfers_allowed
    );
    let (status, body) =
        eng.dispatch_str("POST", "/play", &serde_json::json!({"id": id}).to_string());
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("payload transfers are disabled"), "{body}");
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

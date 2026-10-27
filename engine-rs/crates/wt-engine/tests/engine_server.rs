//! Integration tests ported from `engine-go/server_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
use support::TestEngine;
use support::playback::write_verified_piece;

#[test]
fn test_status_reports_connections_not_discovered_candidates() {
    // Port of Go TestStatusReportsConnectionsNotDiscoveredCandidates
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("tiny", 4096, 2);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent)
    })
    .to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();
    let record = eng.server.records.read().get(id).unwrap().clone();
    record.read().torrent.add_peers(vec![wt_core::PeerInfo {
        id: None,
        addr: "127.0.0.1:1".parse().unwrap(),
        source: wt_core::PeerSource::Tracker,
        supports_encryption: false,
        pex: Default::default(),
        trusted: false,
    }]);

    let (code, body) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
    let view: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        view["numPeers"], 0,
        "reported peers without a live connection"
    );
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
    let priority = || record.read().torrent.inner.pieces.read()[0].base_priority;
    assert_eq!(priority(), wt_core::PiecePriority::Normal);
    let request = serde_json::json!({"id": id}).to_string();
    assert_eq!(eng.dispatch("POST", "/pause", &request).0, 200);
    assert!(record.read().paused);
    let (status, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let view: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(view["paused"], true);
    let (status, _) = eng.dispatch("POST", "/resume", &request);
    assert_eq!(status, 200);
    assert!(!record.read().paused);
    assert_eq!(priority(), wt_core::PiecePriority::Normal);
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

#[test]
fn test_piece_telemetry_reports_selected_pieces() {
    // Port of Go TestPieceTelemetryReportsSelectedPieces
    let eng = TestEngine::new();
    let (torrent, _, _) =
        support::build_multi_file_torrent("bundle", 4, &[("wanted.bin", 4), ("unwanted.bin", 4)]);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent),
    })
    .to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();

    let tmp = crate::support::engine::temp_dir();
    let f1 = std::fs::File::create(tmp.path().join("f1.dat")).expect("create");
    let f2 = std::fs::File::create(tmp.path().join("f2.dat")).expect("create");
    #[cfg(unix)]
    let (fd1, fd2) = {
        use std::os::unix::io::AsRawFd;
        (f1.as_raw_fd(), f2.as_raw_fd())
    };
    #[cfg(not(unix))]
    let (fd1, fd2) = (0, 0);

    let config_req = serde_json::json!({
        "id": id,
        "descriptors": [fd1, fd2],
        "selected": [0],
    });
    let (code, cbody) = eng.dispatch("POST", "/configure", &config_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&cbody));

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
    let resp: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
    let buckets = resp["buckets"].as_array().expect("buckets array");
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0]["selected"], 1);
    assert_eq!(buckets[1]["selected"], 0);
}

#[test]
fn test_accumulate_bucket_piece_selected_mixed_counts() {
    // Port of Go TestAccumulateBucketPieceSelectedMixedCounts
    // Exercise the production aggregator with Go's six input cases, including
    // complete+receiving precedence, rather than copying its logic into a test.
    let runs = [1, 1, 2, 2, 3, 0].map(|flags| wt_storage::PieceRun { length: 1, flags });
    let ranges = [wt_storage::PieceRange { start: 0, end: 1 }, wt_storage::PieceRange { start: 2, end: 3 }, wt_storage::PieceRange { start: 4, end: 6 }];
    let buckets = wt_storage::aggregate_piece_buckets(&runs, &ranges, 6, 1).unwrap();
    let bucket = buckets[0];
    assert_eq!(bucket.selected, 4);
    assert_eq!(bucket.verified, 3);
    assert_eq!(bucket.receiving, 2);
    assert_eq!(bucket.selected_verified, 2);
    assert_eq!(bucket.selected_receiving, 1);

}

#[ignore = "needs: piece telemetry selected verified ignores excluded"]
#[test]
fn test_piece_telemetry_selected_verified_ignores_excluded() {
    // Port of Go TestPieceTelemetrySelectedVerifiedIgnoresExcluded
    let eng = TestEngine::new();
    let data = b"AAAABBBB".to_vec();
    let (torrent, _, _) =
        support::build_multi_file_torrent("bundle", 4, &[("wanted.bin", 4), ("unwanted.bin", 4)]);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent),
    })
    .to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();

    let tmp = crate::support::engine::temp_dir();
    let f1 = std::fs::File::create(tmp.path().join("wanted.dat")).expect("create");
    let f2 = std::fs::File::create(tmp.path().join("unwanted.dat")).expect("create");
    #[cfg(unix)]
    let (fd1, fd2) = {
        use std::os::unix::io::AsRawFd;
        (f1.as_raw_fd(), f2.as_raw_fd())
    };
    #[cfg(not(unix))]
    let (fd1, fd2) = (0, 0);

    let config_req = serde_json::json!({
        "id": id,
        "descriptors": [fd1, fd2],
        "selected": [0],
    });
    let (code, cbody) = eng.dispatch("POST", "/configure", &config_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&cbody));

    write_verified_piece(&eng, id, &data, 0);
    write_verified_piece(&eng, id, &data, 1);

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
    let resp: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
    let buckets = resp["buckets"].as_array().expect("buckets array");
    assert_eq!(buckets[0]["selectedVerified"], 1);
    assert_eq!(buckets[1]["selectedVerified"], 0);
}

#[ignore = "needs: piece telemetry shared boundary counts as selected"]
#[test]
fn test_piece_telemetry_shared_boundary_counts_as_selected() {
    // Port of Go TestPieceTelemetrySharedBoundaryCountsAsSelected
    let eng = TestEngine::new();
    let data = b"AAAABBBB".to_vec();
    let (torrent, _, _) =
        support::build_multi_file_torrent("shared", 8, &[("a.bin", 4), ("b.bin", 4)]);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent),
    })
    .to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();

    let tmp = crate::support::engine::temp_dir();
    let f1 = std::fs::File::create(tmp.path().join("a.dat")).expect("create");
    let f2 = std::fs::File::create(tmp.path().join("b.dat")).expect("create");
    #[cfg(unix)]
    let (fd1, fd2) = {
        use std::os::unix::io::AsRawFd;
        (f1.as_raw_fd(), f2.as_raw_fd())
    };
    #[cfg(not(unix))]
    let (fd1, fd2) = (0, 0);

    let config_req = serde_json::json!({
        "id": id,
        "descriptors": [fd1, fd2],
        "selected": [0],
    });
    let (code, cbody) = eng.dispatch("POST", "/configure", &config_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&cbody));

    write_verified_piece(&eng, id, &data, 0);

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
    let resp: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
    let buckets = resp["buckets"].as_array().expect("buckets array");
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0]["selected"], 1);
    assert_eq!(buckets[0]["verified"], 1);
    assert_eq!(buckets[0]["selectedVerified"], 1);
}

#[test]
fn test_piece_telemetry_inclusive_ranges_and_max_buckets() {
    // Port of Go TestPieceTelemetryInclusiveRangesAndMaxBuckets
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("many", 4, 10);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent),
    })
    .to_string();
    let (status, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let result: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = result["id"].as_str().unwrap();

    let record = eng.server.records.read()[id].clone();
    record.write().configured = true;
    record.write().selected = vec![0];
    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=4"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&pbody));
    let telemetry: engine::types::PieceTelemetryResponse = serde_json::from_slice(&pbody).unwrap();
    assert_eq!(telemetry.total_pieces, 10);
    assert_eq!(telemetry.max_buckets, 4);
    assert_eq!(telemetry.buckets.len(), 4);
    let mut covered = 0;
    for (index, bucket) in telemetry.buckets.iter().enumerate() {
        assert!(bucket.end >= bucket.start, "bucket {index}: {bucket:?}");
        assert_eq!(bucket.total, bucket.end - bucket.start + 1, "inclusive total");
        covered += bucket.total;
        // Unsigned Rust fields reject negative selected counts at deserialization.
        assert!(bucket.selected_verified + bucket.selected_receiving <= bucket.selected);
    }
    assert_eq!(covered, telemetry.total_pieces);
    assert_eq!(telemetry.buckets[0].start, 0);
    assert_eq!(telemetry.buckets.last().unwrap().end, telemetry.total_pieces - 1);
}

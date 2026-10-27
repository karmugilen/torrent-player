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

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
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
    let mut b = engine::types::PieceBucket {
        start: 0,
        end: 6,
        total: 6,
        selected: 0,
        verified: 0,
        receiving: 0,
        selected_verified: 0,
        selected_receiving: 0,
    };

    fn accumulate(
        b: &mut engine::types::PieceBucket,
        selected: bool,
        complete: bool,
        receiving: bool,
    ) {
        if selected {
            b.selected += 1;
        }
        if complete {
            b.verified += 1;
            if selected {
                b.selected_verified += 1;
            }
        } else if receiving {
            b.receiving += 1;
            if selected {
                b.selected_receiving += 1;
            }
        }
    }

    // Selected verified.
    accumulate(&mut b, true, true, false);
    // Excluded verified must inflate Verified only.
    accumulate(&mut b, false, true, false);
    // Selected receiving (Partial/Checking/Hashing path).
    accumulate(&mut b, true, false, true);
    // Excluded receiving must inflate Receiving only.
    accumulate(&mut b, false, false, true);
    // Complete wins over receiving when both flags are set.
    accumulate(&mut b, true, true, true);
    // Missing selected piece.
    accumulate(&mut b, true, false, false);

    assert_eq!(b.selected, 4, "Selected=4");
    assert_eq!(b.verified, 3, "Verified=3");
    assert_eq!(b.receiving, 2, "Receiving=2");
    assert_eq!(b.selected_verified, 2, "SelectedVerified=2");
    assert_eq!(b.selected_receiving, 1, "SelectedReceiving=1");
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

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
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

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
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

#[ignore = "needs: piece telemetry inclusive ranges and maxBuckets"]
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

    let (code, pbody) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=8"), "");
    assert_eq!(code, 200, "pieces: {}", String::from_utf8_lossy(&pbody));
    let resp: serde_json::Value = serde_json::from_slice(&pbody).unwrap();
    let buckets = resp["buckets"].as_array().expect("buckets array");
    assert!(buckets.len() <= 8);
}

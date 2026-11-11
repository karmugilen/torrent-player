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

#[test]
fn test_select_requires_destination_descriptors_and_preserves_nil_response() {
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_multi_file_torrent("selection", 4, &[("first", 4), ("second", 4)]);
    let (_, raw) = eng.dispatch("POST", "/add", &serde_json::json!({
        "prepare": true, "torrentData": base64::engine::general_purpose::STANDARD.encode(torrent),
    }).to_string());
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    let tmp = support::engine::temp_dir();
    let first = std::fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(tmp.path().join("first")).unwrap();
    let second = std::fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(tmp.path().join("second")).unwrap();
    #[cfg(unix)]
    let (fd1, fd2) = {
        use std::os::fd::AsRawFd;
        (first.as_raw_fd(), second.as_raw_fd())
    };
    #[cfg(not(unix))]
    let (fd1, fd2) = (0, 0);
    let select = |selected: serde_json::Value, descriptors: Option<serde_json::Value>| {
        let mut request = serde_json::json!({"id": id, "selected": selected});
        if let Some(descriptors) = descriptors { request["descriptors"] = descriptors; }
        eng.dispatch("POST", "/select", &request.to_string())
    };
    let (code, raw) = eng.dispatch("POST", "/configure", &serde_json::json!({
        "id": id, "descriptors": [fd1, null], "selected": [0],
    }).to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let (code, raw) = select(serde_json::json!([1]), None);
    assert_eq!(code, 400, "{}", String::from_utf8_lossy(&raw));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["error"], "descriptors are required when selecting a file for the first time");
    assert_eq!(eng.server.records.read()[id].read().selected, vec![0]);
    let (code, raw) = select(serde_json::json!([1]), Some(serde_json::json!([null, fd2])));
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    assert_eq!(select(serde_json::json!([1]), None).0, 200);
    for selected in [serde_json::Value::Null, serde_json::json!([])] {
        let (code, raw) = select(selected.clone(), None);
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["selected"], selected);
        assert!(eng.server.records.read()[id].read().selected.is_empty());
    }
}

#[test]
fn test_concurrent_adds_share_one_record_and_remove_readd_gets_new_identity() {
    let eng = TestEngine::new();
    let request = serde_json::json!({"prepare":true,"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789"}).to_string();
    let ids = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8).map(|_| {
            let (eng, request) = (&eng, &request);
            scope.spawn(move || {
                let (code, raw) = eng.dispatch("POST", "/add", request);
                assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
                serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"].as_str().unwrap().to_owned()
            })
        }).collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect::<Vec<_>>()
    });
    assert!(ids.iter().all(|id| id == &ids[0]));
    assert_eq!(ids[0].len(), 36);
    assert_eq!(&ids[0][14..15], "4", "record ID is not a v4 UUID");
    assert_eq!(eng.server.records.read().len(), 1);
    assert_eq!(eng.dispatch("POST", "/remove", &serde_json::json!({"id":ids[0]}).to_string()).0, 200);
    let (code, raw) = eng.dispatch("POST", "/add", &request);
    assert_eq!(code, 200);
    assert_ne!(serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"], ids[0]);
}

#[test]
fn test_add_bytes_raw_torrent_and_pieces_bin_round_trip() {
    let eng = TestEngine::new();
    let (torrent, _, hash) = support::build_single_file_torrent("tiny", 4096, 2);
    let (status, raw) = eng.dispatch_bytes("POST", "/add-bytes?prepare=true", &torrent);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();
    assert_eq!(added["infoHash"].as_str().unwrap(), hash);

    let (json_code, json_body) = eng.dispatch("GET", &format!("/pieces/{id}?maxBuckets=4"), "");
    assert_eq!(json_code, 200, "{}", String::from_utf8_lossy(&json_body));
    let json_telemetry: engine::types::PieceTelemetryResponse =
        serde_json::from_slice(&json_body).unwrap();

    let (bin_code, bin_body) = eng.dispatch("GET", &format!("/pieces-bin/{id}?maxBuckets=4"), "");
    assert_eq!(bin_code, 200, "pieces-bin failed: {}", String::from_utf8_lossy(&bin_body));
    assert_eq!(&bin_body[..4], b"WTPC");
    let decoded = engine::pieces_bin::decode_pieces_bin(&bin_body).expect("decode pieces-bin");
    assert_eq!(decoded.id, json_telemetry.id);
    assert_eq!(decoded.info_hash, json_telemetry.info_hash);
    assert_eq!(decoded.total_pieces, json_telemetry.total_pieces);
    assert_eq!(decoded.buckets, json_telemetry.buckets);
    assert_eq!(decoded.max_buckets, json_telemetry.max_buckets);
}

#[test]
fn test_normalize_magnet_base32_and_hex() {
    let eng = TestEngine::new();
    let hex = "51340689c960f0778a4387aef9b4b52fd08390cd";
    let (code, raw) = eng.dispatch("GET", &format!("/normalize?id=magnet:?xt=urn:btih:{hex}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(body["infoHash"], hex);
    assert!(body["id"].as_str().unwrap().starts_with("magnet:?xt=urn:btih:"));

    let base32 = "ZOCMZQIPFFW7OLLMIC5HUB6BPCSDEOQU";
    let (code, raw) = eng.dispatch(
        "POST",
        "/normalize",
        &serde_json::json!({"id": format!("magnet:?xt=urn:btih:{base32}")}).to_string(),
    );
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let info_hash = body["infoHash"].as_str().unwrap();
    assert_eq!(info_hash.len(), 40);
    assert!(info_hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(body["id"].as_str().unwrap().contains(&format!("btih:{info_hash}")));

    let (code, raw) = eng.dispatch("GET", &format!("/normalize?id={base32}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(body["id"], info_hash);
    assert_eq!(body["infoHash"], info_hash);
}

//! Integration tests ported from `engine-go/telemetry_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

#[test]
fn test_status_fingerprint_ignores_playback_priority_churn() {
    // Port of Go `TestStatusFingerprintIgnoresPlaybackPriorityChurn`.
    let (eng, id, _data) = support::playback_fixture_mode(false);
    wait_until_not_checking(&eng, &id);
    let record = eng.server.records.read().get(&id).unwrap().clone();
    let before = eng.server.status_fingerprint();
    let num_pieces = record.read().torrent.num_pieces();
    assert!(num_pieces >= 2, "fixture too small");

    {
        let rec = record.read();
        rec.torrent.set_piece_priority(0, wt_core::PiecePriority::Now);
        rec.torrent.set_piece_priority(1, wt_core::PiecePriority::Readahead);
    }
    assert!(!record.read().torrent.piece_state_runs().is_empty());
    let after = eng.server.status_fingerprint();
    assert_eq!(after, before, "playback priority churn must not wake Android");
    eng.close();
}

#[test]
fn test_status_fingerprint_wakes_on_verification_and_rates() {
    // Port of Go `TestStatusFingerprintWakesOnVerificationAndRates`.
    let (eng, id, data) = support::playback_fixture_mode(false);
    wait_until_not_checking(&eng, &id);
    let before = eng.server.status_fingerprint();

    support::write_verified_piece(&eng, &id, &data, 0);
    let after_verified = eng.server.status_fingerprint();
    assert_ne!(
        after_verified, before,
        "piece verification must stay observable without PieceStateRuns JSON"
    );

    let before_rates = after_verified;
    eng.server.records.read()[&id].write().downloaded += 1000;
    let after_rates = eng.server.status_fingerprint();
    assert_ne!(
        after_rates, before_rates,
        "rate change must still wake Android"
    );
    eng.close();
}

#[test]
fn test_verified_file_bytes_cache_matches_walk() {
    // Port of Go `TestVerifiedFileBytesCacheMatchesWalk`.
    let (eng, id, data) = support::playback_fixture_mode(false);
    wait_until_not_checking(&eng, &id);
    let (code, body) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(
        code,
        200,
        "status: {code} {}",
        String::from_utf8_lossy(&body)
    );
    let first: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let (code, body) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200);
    let second: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(first["verifiedBytes"], second["verifiedBytes"]);
    assert_eq!(first["files"][1]["verifiedBytes"], second["files"][1]["verifiedBytes"]);

    support::write_verified_piece(&eng, &id, &data, 0);
    let (code, body) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200);
    let after: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let torrent = eng.server.records.read()[&id].read().torrent.clone();
    let info = torrent.info().unwrap();
    for (index, file) in torrent.files().iter().enumerate() {
        let begin = file.offset();
        let end = begin + file.length();
        let mut walked = 0;
        for piece in 0..torrent.num_pieces() {
            if torrent.piece_complete(piece) {
                let start = i64::from(piece) * info.piece_length;
                walked += (end.min(start + info.piece_length) - begin.max(start)).max(0);
            }
        }
        assert_eq!(after["files"][index]["verifiedBytes"], walked, "cache missed file {index} completion");
    }
    eng.close();
}

fn wait_until_not_checking(eng: &support::TestEngine, id: &str) {
    let record = eng.server.records.read()[id].clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while record.read().checking {
        assert!(std::time::Instant::now() < deadline, "torrent stayed in restore-checking");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

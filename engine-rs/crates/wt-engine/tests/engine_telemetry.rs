//! Integration tests ported from `engine-go/telemetry_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;

#[ignore = "needs: status_fingerprint on EngineServer"]
#[test]
fn test_status_fingerprint_ignores_playback_priority_churn() {
    // Port of Go `TestStatusFingerprintIgnoresPlaybackPriorityChurn`.
    let (eng, id, _data) = support::playback_fixture_mode(false);
    let record = eng.server.records.read().get(&id).unwrap().clone();
    let num_pieces = record.read().torrent.num_pieces();
    assert!(num_pieces >= 2, "fixture too small");
    eng.close();
}

#[ignore = "needs: status_fingerprint on EngineServer"]
#[test]
fn test_status_fingerprint_wakes_on_verification_and_rates() {
    // Port of Go `TestStatusFingerprintWakesOnVerificationAndRates`.
    let (eng, id, data) = support::playback_fixture_mode(false);
    support::write_verified_piece(&eng, &id, &data, 0);
    eng.close();
}

#[ignore = "needs: verifiedFileBytes tracking in GET /torrent/{id}"]
#[test]
fn test_verified_file_bytes_cache_matches_walk() {
    // Port of Go `TestVerifiedFileBytesCacheMatchesWalk`.
    let (eng, id, data) = support::playback_fixture_mode(false);
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

    support::write_verified_piece(&eng, &id, &data, 0);
    let (code, body) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200);
    let after: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(after["verifiedBytes"].as_i64().unwrap_or(0) > 0);
    eng.close();
}

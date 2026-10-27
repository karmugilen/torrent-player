//! Integration tests ported from `engine-go/delivery_regression_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "needs: preview read from verified storage tail"]
#[test]
fn test_delivery_preview_can_reuse_verified_tail() {
    // Port of Go `TestDeliveryPreviewCanReuseVerifiedTail`.
    let (eng, id, piece_len, data) = support::large_playback_fixture_mode(24, true);
    support::write_verified_piece(&eng, &id, &data, 0);
    support::write_verified_piece(&eng, &id, &data, 23);
    let p = eng.server.open_preview(&id, 0).expect("open_preview");
    let mut buf = [0u8; 8];
    let n = p.read_at(&mut buf, 23 * piece_len).expect("read_at");
    assert_eq!(n, buf.len(), "already verified tail must remain readable");
    p.close();
}

#[ignore = "needs: persist/load resume window"]
#[test]
fn test_delivery_warm_cache_preserves_explicit_play_from_start() {
    // Port of Go `TestDeliveryWarmCachePreservesExplicitPlayFromStart`.
    let (eng, id, _piece_len, data) = support::large_playback_fixture_mode(24, true);
    for index in [10, 11, 12, 13] {
        support::write_verified_piece(&eng, &id, &data, index);
    }
    let body = serde_json::json!({
        "id": id,
        "fileIndex": 0,
        "offset": 0,
    })
    .to_string();
    let (code, resp) = eng.dispatch("POST", "/play", &body);
    assert_eq!(
        code,
        200,
        "play from start: {code} {}",
        String::from_utf8_lossy(&resp)
    );
    let p = eng.server.open_playback(&id, 0).expect("open_playback");
    assert_eq!(
        p.current_offset.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "cache changed explicit start position"
    );
    p.close();
}

#[test]
fn test_delivery_concurrent_cache_clears() {
    // Port of Go `TestDeliveryConcurrentCacheClears`.
    let eng = std::sync::Arc::new(TestEngine::new());
    let mut handles = Vec::new();
    for _ in 0..4 {
        let eng = eng.clone();
        handles.push(std::thread::spawn(move || {
            let req = serde_json::json!({"categories": ["peers", "dht"]}).to_string();
            for _ in 0..20 {
                let (code, _) = eng.dispatch("POST", "/cache/clear", &req);
                assert_eq!(code, 200);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[ignore = "needs: preview expiration task to clear preview_active"]
#[test]
fn test_delivery_expired_preview_releases_slot() {
    // Port of Go `TestDeliveryExpiredPreviewReleasesSlot`.
    let (_data, metadata) = support::recovery_metadata(None);
    let eng = TestEngine::new();
    let id = support::add_recovery_torrent(&eng, &metadata);
    let p = eng.server.open_preview(&id, 0).expect("open_preview");
    p.close();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    let mut released = false;
    while std::time::Instant::now() < deadline {
        if eng.server.preview_active.lock().is_none() {
            released = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        released,
        "expired preview still owns slot and temporary storage/demand"
    );
}

#[ignore = "needs: preview temporary piece discarding"]
#[test]
fn test_preview_release_invalidates_temporary_completion() {
    // Port of Go `TestPreviewReleaseInvalidatesTemporaryCompletion`.
    let (data, metadata) = support::recovery_metadata(None);
    let eng = TestEngine::new();
    let id = support::add_recovery_torrent(&eng, &metadata);
    let p = eng.server.open_preview(&id, 0).expect("open_preview");
    support::write_verified_piece(&eng, &id, &data, 0);
    let rec = eng.server.records.read().get(&id).unwrap().clone();
    assert!(
        rec.read().torrent.piece_complete(0),
        "fixture piece not verified"
    );
    p.close();
    assert!(
        !rec.read().torrent.piece_complete(0),
        "discarded preview bytes still reported complete"
    );
}

#[ignore = "needs: preview read to check payload gate / admit_preview_range"]
#[test]
fn test_preview_honors_payload_gate() {
    // Port of Go `TestPreviewHonorsPayloadGate`.
    let (_data, metadata) = support::recovery_metadata(None);
    let eng = TestEngine::new();
    let id = support::add_recovery_torrent(&eng, &metadata);
    eng.server
        .payload_transfers_allowed
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let p = match eng.server.open_preview(&id, 0) {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut buf = [0u8; 8];
    let res = p.read_at(&mut buf, 0);
    assert!(res.is_err(), "preview bypassed payload policy");
    p.close();
}

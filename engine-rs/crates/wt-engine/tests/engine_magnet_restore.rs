//! Integration tests ported from `engine-go/magnet_restore_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_pending_magnet_can_resume_before_storage_is_configured() {
    // Port of Go `TestPendingMagnetCanResumeBeforeStorageIsConfigured`.
    let eng = TestEngine::new();
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        r#"{"torrentId":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","prepare":true}"#,
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();

    let pause_req = serde_json::json!({"id": id}).to_string();
    let (code, body) = eng.dispatch("POST", "/pause", &pause_req);
    assert_eq!(
        code,
        200,
        "pause before metadata: {code} {}",
        String::from_utf8_lossy(&body)
    );

    let (code, body) = eng.dispatch("POST", "/resume", &pause_req);
    assert_eq!(
        code,
        200,
        "resume before metadata: {code} {}",
        String::from_utf8_lossy(&body)
    );

    let record = eng.server.records.read().get(id).unwrap().clone();
    let rec = record.read();
    assert!(!rec.paused, "must not remain paused");
    assert!(!rec.configured, "must not be configured yet");
    assert!(rec.torrent.info().is_none(), "info must still be pending");
    assert_eq!(rec.torrent.stats().conn_stats.bytes_read_data, 0);
    drop(rec);
    eng.close();
}

#[test]
fn test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata() {
    use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
    use std::time::{Duration, Instant};
    use base64::Engine as _;
    let seed = TestEngine::new();
    let bytes = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/tiny.torrent")).unwrap();
    let (mut mi, _) = wt_meta::parse_torrent(&bytes).unwrap();
    mi.announce = String::new();
    mi.announce_list.clear();
    let metadata = wt_meta::encode_metainfo(&mi).unwrap();
    let (code, raw) = seed.dispatch("POST", "/add", &serde_json::json!({"prepare":true,"torrentData":base64::engine::general_purpose::STANDARD.encode(metadata)}).to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let port = seed.server.client.local_port();
    assert_ne!(port, 0, "missing seed listener");
    let compact = [127,0,0,1,(port>>8) as u8,port as u8];
    let available = Arc::new(AtomicBool::new(false));
    let announces = Arc::new(AtomicUsize::new(0));
    let (active, count) = (available.clone(), announces.clone());
    let tracker = support::http::HttpServer::new(move |_, stream| {
        count.fetch_add(1, Ordering::SeqCst);
        let mut body = b"d8:intervali1e5:peers".to_vec();
        if active.load(Ordering::SeqCst) { body.extend_from_slice(b"6:"); body.extend_from_slice(&compact); }
        else { body.extend_from_slice(b"0:"); }
        body.push(b'e');
        support::http::respond(stream, 200, "", &body);
    });
    let mut source = url::Url::parse(&format!("magnet:?xt=urn:btih:{}", wt_meta::info_hash_v1(&mi.info_bytes).to_hex())).unwrap();
    source.query_pairs_mut().append_pair("tr", &format!("{}/announce", tracker.url));
    let add = |eng: &TestEngine| {
        let (code, raw) = eng.dispatch("POST", "/add", &serde_json::json!({"prepare":true,"torrentId":source.as_str()}).to_string());
        assert_eq!(code, 200, "add pending: {}", String::from_utf8_lossy(&raw));
        serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"].as_str().unwrap().to_owned()
    };
    let before = TestEngine::new();
    let pending = add(&before);
    assert!(before.server.records.read()[&pending].read().torrent.info().is_none(), "expected pending before restart");
    before.close();
    available.store(true, Ordering::SeqCst);
    let restored = TestEngine::new();
    let id = add(&restored);
    assert_ne!(id, pending, "cold restart reused identity");
    let torrent = restored.server.records.read()[&id].read().torrent.clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    while torrent.info().is_none() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
    assert!(torrent.info().is_some(), "cold restore failed tracker/metadata exchange; announces={}", announces.load(Ordering::SeqCst));
    let (code, body) = restored.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
    let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status["ready"], true);
    assert_eq!(status["paused"], false);
    assert_eq!(status["configured"], false);
    assert!(!status["files"].as_array().unwrap().is_empty());
    assert!(announces.load(Ordering::SeqCst) > 0);
    assert_eq!(torrent.stats().conn_stats.bytes_read_data, 0, "payload downloaded before configure");
    for file in torrent.files() { assert_eq!(file.priority(), wt_core::PiecePriority::None, "discovery enabled payload priority"); }
}

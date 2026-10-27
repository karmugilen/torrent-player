//! Integration tests ported from `engine-go/settings_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use base64::Engine as _;
use support::TestEngine;

#[test]
fn test_settings_apply_to_existing_and_future_torrents() {
    let eng = TestEngine::new();
    let add = |name: &str| {
        let (torrent, _, _) = support::build_single_file_torrent(name, 4096, 2);
        let encoded = base64::engine::general_purpose::STANDARD.encode(torrent);
        let (code, raw) = eng.dispatch(
            "POST",
            "/add",
            &serde_json::json!({"prepare":true,"torrentData":encoded}).to_string(),
        );
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
        let response: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        response["id"].as_str().unwrap().to_owned()
    };
    let first = add("tiny");
    let settings = eng.post_settings(r#"{"maxPeers":500,"ramCacheBytes":536870912}"#);
    assert_eq!(settings.max_peers, 500);
    assert_eq!(settings.ram_cache_bytes, 512 << 20);
    let second = add("next");
    assert_ne!(first, second);
    for id in [&first, &second] {
        let record = eng.server.records.read().get(id).unwrap().clone();
        let rec = record.read();
        assert_eq!(
            rec.torrent.set_max_established_conns(500),
            500,
            "torrent {id}"
        );
        let storage = eng
            .server
            .storage
            .get_storage(&rec.info_hash)
            .expect("torrent storage");
        assert_eq!(storage.cache_limit(), 512 << 20);
    }
    eng.post_settings(r#"{"maxPeers":55}"#);
    assert_eq!(eng.server.storage.cache_limit(), 512 << 20);
    for id in [&first, &second] {
        let record = eng.server.records.read().get(id).unwrap().clone();
        assert_eq!(record.read().torrent.set_max_established_conns(55), 55);
    }
    for invalid in [
        r#"{"maxPeers":501}"#,
        r#"{"maxPeers":7}"#,
        r#"{"ramCacheBytes":16777215}"#,
        r#"{"ramCacheBytes":1073741825}"#,
    ] {
        assert_eq!(
            eng.dispatch("POST", "/settings", invalid).0,
            400,
            "{invalid}"
        );
    }
    for limit in [16_i64 << 20, 64 << 20, 1024 << 20] {
        assert_eq!(
            eng.dispatch(
                "POST",
                "/settings",
                &format!(r#"{{"ramCacheBytes":{limit}}}"#)
            )
            .0,
            200
        );
    }
}

#[ignore = "waiting for storage/cache integration"]
#[test]
fn test_cache_shrink_preserves_dirty_download_bytes() {
    // Port of Go `TestCacheShrinkPreservesDirtyDownloadBytes`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestCacheShrinkPreservesDirtyDownloadBytes");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_cache_resize_keeps_direct_play_readable_and_preview_bounded() {
    // Port of Go `TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded");
}

#[ignore = "waiting for playback/storage/core integration"]
#[test]
fn test_small_ram_setting_keeps_four_direct_play_pieces() {
    // Port of Go `TestSmallRAMSettingKeepsFourDirectPlayPieces`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestSmallRAMSettingKeepsFourDirectPlayPieces");
}

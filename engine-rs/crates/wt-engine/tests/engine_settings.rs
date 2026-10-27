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

#[test]
fn test_cache_shrink_preserves_dirty_download_bytes() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    use wt_storage::ClientStorage;
    let tmp = support::engine::temp_dir();
    let (torrent, _, _) = support::build_single_file_torrent("data", 4, 3);
    let (mi, info) = wt_meta::parse_torrent(&torrent).unwrap();
    let hash = wt_meta::info_hash_v1(&mi.info_bytes);
    let client = wt_storage::DocumentStorageClient::new(tmp.path());
    client.set_cache_limit(8).unwrap();
    let implementation = client.open_torrent(&info, hash).unwrap();
    let st = client.get_storage(&hash.to_hex()).unwrap();
    let destination = std::fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(tmp.path().join("download")).unwrap();
    st.attach_descriptors(&[Some(destination.as_raw_fd())], &[0]).unwrap();
    for (index, payload) in [b"AAAA", b"BBBB", b"CCCC"].iter().enumerate() {
        if index == 2 { client.set_cache_limit(4).unwrap(); }
        assert_eq!(st.piece_at(index).write_at(*payload, 0).unwrap(), 4);
    }
    let mut got = [0; 8];
    destination.read_exact_at(&mut got, 0).unwrap();
    assert_eq!(&got, b"AAAABBBB", "eviction lost dirty data");
    assert!(st.cache_bytes() <= st.max_cache_bytes(), "cache exceeds new cap");
    implementation.close().unwrap();
}

#[test]
fn test_cache_resize_keeps_direct_play_readable_and_preview_bounded() {
    let tmp = support::engine::temp_dir();
    let (torrent, data, _) = support::build_single_file_torrent("video", 1 << 20, 12);
    let (mi, info) = wt_meta::parse_torrent(&torrent).unwrap();
    let hash = wt_meta::info_hash_v1(&mi.info_bytes);
    let st = wt_storage::DocumentTorrentStorage::new(Some(info.clone()), hash, tmp.path());
    st.attach_descriptors_mode(&[None], &[0], true).unwrap();
    let piece = st.piece_at(6);
    piece.write_at(&data[6 << 20..7 << 20], 0).unwrap();
    piece.mark_complete().unwrap();
    st.set_cache_limit(128 << 20);
    let mut got = [0; 32];
    let (n, _, error) = st.read_verified_stream(&mut got, 6 << 20);
    error.unwrap();
    assert_eq!(n, got.len());
    assert_eq!(got, data[6 << 20..(6 << 20) + 32]);
    let preview = wt_storage::DocumentTorrentStorage::new(Some(info), hash, tmp.path());
    preview.attach_preview_temp(0).unwrap();
    preview.set_cache_limit(256 << 20);
    assert_eq!(preview.max_cache_bytes(), wt_storage::PREVIEW_BUDGET_BYTES, "setting expanded preview budget");
    preview.release_preview_temp();
    assert_eq!(preview.max_cache_bytes(), 256 << 20, "release lost configured budget");
}

#[test]
fn test_small_ram_setting_keeps_four_direct_play_pieces() {
    use wt_storage::ClientStorage;
    let tmp = support::engine::temp_dir();
    let info = wt_types::Info {
        name: "large-pieces".into(), piece_length: 32 << 20, pieces: vec![0;160],
        files: vec![wt_types::FileInfo { length: 256 << 20, ..Default::default() }],
        ..Default::default()
    };
    let hash = wt_types::InfoHash::default();
    let client = wt_storage::DocumentStorageClient::new(tmp.path());
    client.set_cache_limit(16 << 20).unwrap();
    let implementation = client.open_torrent(&info, hash).unwrap();
    let st = client.get_storage(&hash.to_hex()).unwrap();
    st.attach_descriptors_mode(&[None], &[0], true).expect("16 MiB setting refused 32 MiB pieces");
    assert_eq!(st.max_cache_bytes(), 128 << 20, "must retain four pieces");
    client.set_cache_limit(512 << 20).unwrap();
    assert_eq!(st.max_cache_bytes(), 512 << 20);
    client.set_cache_limit(16 << 20).expect("shrinking below four pieces refused");
    assert_eq!(client.cache_limit(), 16 << 20);
    assert_eq!(st.max_cache_bytes(), 128 << 20);
    implementation.close().unwrap();
}

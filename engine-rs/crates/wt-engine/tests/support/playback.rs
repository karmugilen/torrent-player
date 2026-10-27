//! Shared playback test fixtures mirroring Go's `playback_test.go` and `verification_test.go`.

use crate::support::{self, TestEngine};
use base64::Engine as _;

/// Mirrors Go `largePlaybackFixtureMode`.
pub fn large_playback_fixture_mode(
    pieces: usize,
    direct: bool,
) -> (TestEngine, String, i64, Vec<u8>) {
    let eng = TestEngine::new();
    let piece_len = 1024 * 1024i64;
    let (torrent, data, _hash) = support::build_single_file_torrent("movie.mp4", piece_len, pieces);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap().to_owned();

    let configure_req = if direct {
        serde_json::json!({
            "id": id,
            "descriptors": [null],
            "storageMode": "stream",
            "streamCacheBytes": 16 << 20,
            "selected": [0],
        })
    } else {
        let tmp = tempfile::tempdir_in(
            std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
        )
        .expect("tempdir");
        let file_path = tmp.path().join("movie.dat");
        let f = std::fs::File::create(&file_path).expect("create");
        #[cfg(unix)]
        let fd = {
            use std::os::unix::io::AsRawFd;
            f.as_raw_fd()
        };
        #[cfg(not(unix))]
        let fd = 0;
        serde_json::json!({
            "id": id,
            "descriptors": [fd],
            "selected": [0],
        })
    };
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    (eng, id, piece_len, data)
}

/// Two files sharing a piece: readme.txt (5 bytes) and clip.mp4 (remainder).
pub fn playback_fixture_mode(direct: bool) -> (TestEngine, String, Vec<u8>) {
    let eng = TestEngine::new();
    let data = b"note!\x00\x00\x00\x18ftypmp42ABCDEFGHIJKLMN0123456789".to_vec();
    let piece_length = 8i64;
    let (torrent, _, _) = support::build_multi_file_torrent(
        "media",
        piece_length,
        &[("readme.txt", 5), ("clip.mp4", (data.len() - 5) as i64)],
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap().to_owned();

    let configure_req = if direct {
        serde_json::json!({
            "id": id,
            "descriptors": [null, null],
            "storageMode": "stream",
            "streamCacheBytes": 16 << 20,
            "selected": [1],
        })
    } else {
        let tmp = tempfile::tempdir_in(
            std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
        )
        .expect("tempdir");
        let file_path = tmp.path().join("clip.dat");
        let f = std::fs::File::create(&file_path).expect("create");
        #[cfg(unix)]
        let fd = {
            use std::os::unix::io::AsRawFd;
            f.as_raw_fd()
        };
        #[cfg(not(unix))]
        let fd = 0;
        serde_json::json!({
            "id": id,
            "descriptors": [null, fd],
            "selected": [1],
        })
    };
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    (eng, id, data)
}

/// Mirrors Go `writeVerifiedPiece`.
pub fn write_verified_piece(eng: &TestEngine, id: &str, data: &[u8], index: usize) {
    let record = eng.server.records.read().get(id).unwrap().clone();
    let rec = record.read();
    let info = rec.torrent.info().expect("info");
    let start = index as i64 * info.piece_length;
    let end = (start + info.piece_length).min(data.len() as i64);
    let piece_bytes = &data[start as usize..end as usize];
    let info_hash = rec.info_hash.clone();
    drop(rec);

    if let Some(st) = eng.server.storage.get_storage(&info_hash) {
        let doc_piece = st.piece_at(index);
        let _ = doc_piece.write_at(piece_bytes, 0);
        let _ = doc_piece.mark_complete();
        st.set_piece_complete(index, true);
    }
    let rec = record.read();
    rec.torrent.update_piece_completion(index as u32);
}

/// Mirrors Go `recoveryMetadata`.
pub fn recovery_metadata(repeats: Option<usize>) -> (Vec<u8>, String) {
    let count = repeats.unwrap_or(8192);
    let motif = b"saved torrent data.";
    let mut data = Vec::with_capacity(motif.len() * count);
    for _ in 0..count {
        data.extend_from_slice(motif);
    }
    let piece_length = 16 * 1024i64;
    let piece_count = data.len().div_ceil(piece_length as usize);
    let (torrent, _, _) =
        support::build_single_file_torrent("resume.bin", piece_length, piece_count);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    (data, encoded)
}

/// Mirrors Go `addRecoveryTorrent`.
pub fn add_recovery_torrent(eng: &TestEngine, metadata: &str) -> String {
    let body = serde_json::json!({"prepare": true, "torrentData": metadata}).to_string();
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    added["id"].as_str().unwrap().to_owned()
}

/// Mirrors Go `metadataEpisodes`.
pub fn metadata_episodes(
    base: &std::path::Path,
) -> (
    std::sync::Arc<wt_storage::DocumentTorrentStorage>,
    wt_types::Info,
    Vec<u8>,
) {
    let payload = support::piece_payload(1 << 20);
    let piece_length = 1 << 20i64;
    let file_length = 12 << 20i64;
    let files = &[
        ("e1.mkv", file_length),
        ("e2.mkv", file_length),
        ("e3.mkv", file_length),
        ("e4.mkv", file_length),
        ("notes.txt", file_length),
    ];
    let (torrent, _, _) = support::build_multi_file_torrent("season", piece_length, files);
    let (mi, _) = wt_meta::parse_torrent(&torrent).expect("parse");
    let info = wt_meta::parse_info(&mi.info_bytes).expect("info");
    let info_hash = wt_meta::info_hash_v1(&mi.info_bytes);
    let st = wt_storage::DocumentTorrentStorage::new(Some(info.clone()), info_hash, base);
    st.set_cache_limit(16 << 20);
    st.attach_descriptors_mode(&[None; 5], &[0, 1, 2, 3, 4], true)
        .expect("attach");
    (st, info, payload)
}

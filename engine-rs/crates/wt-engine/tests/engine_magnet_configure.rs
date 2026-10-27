//! Android configures a magnet immediately, while discovery still lacks metadata.
mod support;

use std::sync::{mpsc, Arc};
use std::time::Duration;

use engine::EngineServer;
use serde_json::json;

fn pending_magnet() -> (tempfile::TempDir, Arc<EngineServer>, String, Vec<u8>) {
    let root = "/home/kar/.cache/wt-tmp/codex-engine";
    std::fs::create_dir_all(root).unwrap();
    let dir = tempfile::tempdir_in(root).unwrap();
    let server = EngineServer::new(0, dir.path(), 10).unwrap();
    let (metainfo, _, hash) = support::build_single_file_torrent("movie.mp4", 16384, 2);
    let (status, body) = server.request(
        "POST",
        "/add",
        &json!({
            "torrentId": format!("magnet:?xt=urn:btih:{hash}")
        })
        .to_string(),
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(server
        .record_by_id(&hash)
        .unwrap()
        .read()
        .torrent
        .info()
        .is_none());
    (dir, server, hash, metainfo)
}

#[test]
fn immediate_configure_waits_for_magnet_metadata() {
    let (_dir, server, hash, metainfo) = pending_magnet();
    let (tx, rx) = mpsc::channel();
    let worker = server.clone();
    let body = json!({"id": hash, "selected": [0], "descriptors": [null]}).to_string();
    let join = std::thread::spawn(move || {
        tx.send(worker.request("POST", "/configure", &body))
            .unwrap();
    });
    assert!(
        matches!(
            rx.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "configure must wait while metadata is unavailable"
    );
    // Both the records map and the record must remain available while configure waits.
    let rec = server
        .records
        .try_read()
        .expect("records lock held during wait")
        .get(&hash)
        .unwrap()
        .clone();
    assert!(rec.try_write().is_some(), "record lock held during wait");
    let meta = wt_meta::parse_metainfo(&metainfo).unwrap();
    server
        .client
        .add_torrent_spec(wt_core::TorrentSpec {
            info_hash: Some(wt_types::InfoHash::from_hex(&hash).unwrap()),
            info_bytes: Some(meta.info_bytes),
            ..Default::default()
        })
        .unwrap();
    let (status, body) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(rec.read().configured);
    assert!(server.storage.get_storage(&hash).is_some());
    join.join().unwrap();
    server.close();
}

#[test]
fn metadata_wait_returns_conflict_when_magnet_is_removed() {
    for route in ["/configure", "/play", "/select"] {
        let (_dir, server, hash, _) = pending_magnet();
        let (tx, rx) = mpsc::channel();
        let worker = server.clone();
        let body = json!({"id": hash, "selected": [0], "descriptors": [null]}).to_string();
        let join = std::thread::spawn(move || {
            tx.send(worker.request("POST", route, &body)).unwrap();
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            server
                .request("POST", "/remove", &json!({"id": hash}).to_string())
                .0,
            200
        );
        let (status, body) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(status, 409, "{route}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "torrent removed",
            "{route}"
        );
        join.join().unwrap();
        server.close();
    }
}

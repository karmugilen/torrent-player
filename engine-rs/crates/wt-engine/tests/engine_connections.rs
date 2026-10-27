//! Integration tests ported from `engine-go/connections_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[test]
fn test_offline_peer_countries() {
    // Port of Go `TestOfflinePeerCountries`.
    let tmp = crate::support::engine::temp_dir();
    let download_dir = tmp.path().to_path_buf();
    let mmdb_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../android/app/src/main/assets/geoip-country-2026-10.mmdb");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&mmdb_src, download_dir.join("geoip-country-2026-10.mmdb"))
        .expect("symlink geoip");

    let server = engine::EngineServer::new(0, &download_dir, 55).expect("EngineServer::new");
    for (address, expected) in [
        ("8.8.8.8:6881", "US"),
        ("[2001:4860:4860::8888]:6881", "CA"),
    ] {
        let country = server.geoip.lookup_country(address).unwrap_or_default();
        assert_eq!(country, expected, "country for {address}");
    }
    for address in [
        "127.0.0.1:6881",
        "192.168.1.2:6881",
        "[::1]:6881",
        "[fc00::1]:6881",
        "WebRTC peer",
        "invalid",
    ] {
        let country = server.geoip.lookup_country(address).unwrap_or_default();
        assert_eq!(country, "", "invented country for {address}");
    }
    server.close();
}

use base64::Engine as _;
use engine::types::ConnectionsResponse;

fn connection_snapshot(eng: &TestEngine, id: &str) -> ConnectionsResponse {
    let (code, raw) = eng.dispatch("GET", &format!("/connections/{id}"), "");
    assert_eq!(code, 200, "connections: {}", String::from_utf8_lossy(&raw));
    let json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(json["peers"].is_array(), "empty peer list must be an array");
    let resp: ConnectionsResponse = serde_json::from_value(json).unwrap();
    resp
}

#[test]
fn test_connections_exclude_candidates_and_unknown_torrent() {
    // Port of Go `TestConnectionsExcludeCandidatesAndUnknownTorrent`.
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("tiny", 4096, 2);
    let encoded = base64::engine::general_purpose::STANDARD.encode(torrent);
    let (code, raw) = eng.dispatch(
        "POST",
        "/add",
        &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
    );
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let response: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = response["id"].as_str().unwrap().to_owned();

    let record = eng.server.records.read().get(&id).unwrap().clone();
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    record.read().torrent.add_peers(vec![wt_types::PeerInfo {
        id: None,
        addr: std::net::SocketAddr::new(ip, 1),
        source: wt_types::PeerSource::Tracker,
        supports_encryption: false,
        pex: wt_types::PexFlags::default(),
        trusted: false,
    }]);

    let view = connection_snapshot(&eng, &id);
    assert!(
        view.peers.is_empty(),
        "discovered candidates rendered as connected: {:?}",
        view.peers
    );

    let (code, _) = eng.dispatch("GET", "/connections/missing", "");
    assert_eq!(code, 404, "missing torrent status {code}");
    eng.close();
}

#[test]
fn test_connections_reports_real_tcp_peer_and_clears_on_pause() {
    // Port of Go `TestConnectionsReportsRealTCPPeerAndClearsOnPause`.
    let new_peer = |name: &str| -> (TestEngine, String) {
        let eng = TestEngine::new();
        let (torrent, _, _) = support::build_single_file_torrent("tiny", 4096, 2);
        let encoded = base64::engine::general_purpose::STANDARD.encode(torrent);
        let (code, raw) = eng.dispatch(
            "POST",
            "/add",
            &serde_json::json!({"prepare": true, "torrentData": encoded}).to_string(),
        );
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
        let response: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let id = response["id"].as_str().unwrap().to_owned();

        let tmp = crate::support::engine::temp_dir();
        let file_path = tmp.path().join(format!("{name}.dat"));
        let f = std::fs::File::create(&file_path).expect("create");
        #[cfg(unix)]
        let fd = {
            use std::os::unix::io::AsRawFd;
            f.as_raw_fd()
        };
        #[cfg(not(unix))]
        let fd = 0;
        let configure_req = serde_json::json!({
            "id": id,
            "descriptors": [fd],
            "selected": [0],
        });
        let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
        assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
        (eng, id)
    };

    let (remote_eng, remote_id) = new_peer("remote");
    let (eng, id) = new_peer("local");

    let _remote_rec = remote_eng
        .server
        .records
        .read()
        .get(&remote_id)
        .unwrap()
        .clone();

    support::policy::connect(&eng, &id, &remote_eng);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let view = connection_snapshot(&eng, &id);
        if !view.peers.is_empty() {
            let peer = &view.peers[0];
            let host = peer.address.split(':').next().unwrap();
            assert_eq!(host, "127.0.0.1");
            assert_eq!(peer.transport, "TCP");
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("local peer did not connect");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let pause_req = serde_json::json!({"id": id}).to_string();
    let (code, body) = eng.dispatch("POST", "/pause", &pause_req);
    assert_eq!(code, 200, "pause: {}", String::from_utf8_lossy(&body));
    let view = connection_snapshot(&eng, &id);
    assert!(view.paused);
    assert!(
        view.peers.is_empty(),
        "paused map retains connections: {:?}",
        view.peers
    );

    eng.close();
    remote_eng.close();
}

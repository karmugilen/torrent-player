//! Server policy regressions; no public swarm or peer traffic is needed.
use super::*;

pub(super) fn test_server() -> (tempfile::TempDir, Arc<EngineServer>) {
    let root = "/home/kar/.cache/wt-tmp/codex-engine";
    std::fs::create_dir_all(root).unwrap();
    let dir = tempfile::tempdir_in(root).unwrap();
    let server = EngineServer::new_with_config(0, dir.path(), 55, |config| {
        config.no_dht = true;
        config.disable_utp = true;
        config.disable_webtorrent = true;
        config.dial_for_peer_conns = false;
    })
    .unwrap();
    (dir, server)
}

fn add_torrent(server: &EngineServer) -> Arc<RwLock<TorrentRecord>> {
    let info_bytes = wt_meta::encode_info(&wt_types::Info {
        name: "policy.bin".into(),
        piece_length: 16384,
        pieces: vec![0; 40],
        files: vec![wt_types::FileInfo {
            length: 32768,
            ..Default::default()
        }],
        ..Default::default()
    })
    .unwrap();
    let hash = wt_meta::info_hash_v1(&info_bytes).to_hex();
    let bytes = wt_meta::encode_metainfo(&wt_types::MetaInfo {
        info_bytes,
        ..Default::default()
    })
    .unwrap();
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    assert_eq!(
        server
            .request(
                "POST",
                "/add",
                &json!({"prepare":true,"torrentData":data}).to_string()
            )
            .0,
        200
    );
    let rec = server.records_by_hash.read().get(&hash).unwrap().clone();
    {
        let mut record = rec.write();
        record.configured = true;
        record.selected = vec![0];
        server.apply_transfer_state(&mut record);
    }
    rec
}

fn settings(server: &EngineServer, body: serde_json::Value) -> serde_json::Value {
    let (status, response) = server.request("POST", "/settings", &body.to_string());
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&response));
    serde_json::from_slice(&response).unwrap()
}

#[test]
fn connection_defaults_scale_half_open_with_max_peers() {
    let (_dir, server) = test_server();
    let config = &server.client.inner.config;
    assert_eq!(config.established_conns_per_torrent, 55);
    assert_eq!(config.half_open_conns_per_torrent, 55);
    assert_eq!(config.total_half_open_conns, 110);
    assert_eq!(
        server
            .client
            .inner
            .half_open_conns_per_torrent
            .load(Ordering::Relaxed),
        55
    );
    assert_eq!(config.nominal_dial_timeout, Duration::from_secs(8));
    assert_eq!(config.min_dial_timeout, Duration::from_secs(2));
    assert_eq!(config.handshakes_timeout, Duration::from_secs(5));
    assert_eq!(config.dial_rate, 30.0);
    let rec = add_torrent(&server);
    settings(&server, json!({"maxPeers":128}));
    assert_eq!(rec.read().torrent.set_max_established_conns(128), 128);
    assert_eq!(
        server
            .client
            .inner
            .half_open_conns_per_torrent
            .load(Ordering::Relaxed),
        128
    );
    assert_eq!(
        server
            .client
            .inner
            .total_half_open_conns
            .load(Ordering::Relaxed),
        256
    );
    server.close();
}

#[test]
fn rejected_settings_do_not_partially_change_peer_limits() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    assert_eq!(
        server
            .request("POST", "/settings", r#"{"maxPeers":500,"ramCacheBytes":1}"#)
            .0,
        400
    );
    assert_eq!(server.max_peers.load(Ordering::SeqCst), 55);
    assert_eq!(rec.read().torrent.set_max_established_conns(55), 55);
    server.close();
}

#[test]
fn unrelated_settings_preserve_urgent_piece_demand_and_resting_peer_cap() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    {
        let mut record = rec.write();
        record
            .torrent
            .set_piece_priority(0, wt_core::PiecePriority::Now);
        record.resting = true;
        record.torrent.set_max_established_conns(0);
    }
    settings(
        &server,
        json!({"maxPeers":88,"dialRatePerSec":40,"ramCacheBytes":33554432}),
    );
    assert_eq!(
        rec.read().torrent.piece_state(0).priority,
        wt_core::PiecePriority::Now
    );
    assert_eq!(rec.read().torrent.set_max_established_conns(0), 0);
    assert_eq!(server.client.inner.dial_limiter.limit(), 40.0);
    assert_eq!(server.client.inner.dial_limiter.burst(), 40);
    server.close();
}

#[test]
fn transfer_settings_ignore_negative_rates_and_keep_go_bursts() {
    let (_dir, server) = test_server();
    settings(
        &server,
        json!({"downloadRateBytesSec":65536,"uploadRateBytesSec":4096}),
    );
    assert_eq!(server.client.inner.download_limiter.burst(), 65536);
    assert_eq!(server.client.inner.upload_limiter.burst(), 4096);
    let reply = settings(
        &server,
        json!({"downloadRateBytesSec":-1,"uploadRateBytesSec":-1}),
    );
    assert_eq!(reply["downloadRateBytesSec"], 65536);
    assert_eq!(reply["uploadRateBytesSec"], 4096);
    settings(
        &server,
        json!({"downloadRateBytesSec":0,"uploadRateBytesSec":0}),
    );
    assert!(server.client.inner.download_limiter.limit().is_infinite());
    assert!(server.client.inner.upload_limiter.limit().is_infinite());
    assert_eq!(server.client.inner.download_limiter.burst(), 65536);
    server.close();
}

#[test]
fn dial_and_seeding_limits_follow_go_bounds() {
    let (_dir, server) = test_server();
    let reply = settings(
        &server,
        json!({"dialRatePerSec":-10,"idleDropMinutes":-1,
        "seedRatioMilli":-1,"seedTimeMinutes":0}),
    );
    assert_eq!(reply["dialRatePerSec"], 10);
    assert_eq!(reply["idleDropMinutes"], 0);
    assert_eq!(reply["seedRatioMilli"], 1000);
    assert_eq!(reply["seedTimeMinutes"], 60);
    let reply = settings(
        &server,
        json!({"dialRatePerSec":999,"idleDropMinutes":999,
        "seedRatioMilli":10000,"seedTimeMinutes":1440}),
    );
    assert_eq!(reply["dialRatePerSec"], 80);
    assert_eq!(reply["idleDropMinutes"], 240);
    assert_eq!(reply["seedRatioMilli"], 10000);
    assert_eq!(reply["seedTimeMinutes"], 1440);
    server.close();
}

#[test]
fn incomplete_selection_keeps_reciprocal_uploads_with_seeding_disabled() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    settings(
        &server,
        json!({"seedingEnabled":false,"seedingAllowed":false}),
    );
    assert!(!rec
        .read()
        .torrent
        .inner
        .data_upload_disallowed
        .load(Ordering::SeqCst));
    assert!(!rec
        .read()
        .torrent
        .inner
        .data_download_disallowed
        .load(Ordering::SeqCst));
    assert!(!rec.read().seeding);
    settings(&server, json!({"payloadTransfersAllowed":false}));
    assert!(rec
        .read()
        .torrent
        .inner
        .data_upload_disallowed
        .load(Ordering::SeqCst));
    assert!(rec
        .read()
        .torrent
        .inner
        .data_download_disallowed
        .load(Ordering::SeqCst));
    server.close();
}

#[test]
fn checking_selection_never_enables_payload_on_policy_updates() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    rec.write().checking = true;
    settings(&server, json!({"payloadTransfersAllowed":true}));
    let record = rec.read();
    assert!(record
        .torrent
        .inner
        .data_upload_disallowed
        .load(Ordering::SeqCst));
    assert!(record
        .torrent
        .inner
        .data_download_disallowed
        .load(Ordering::SeqCst));
    assert_eq!(
        record.torrent.piece_state(0).priority,
        wt_core::PiecePriority::None
    );
    drop(record);
    server.close();
}

#[test]
fn control_routes_handle_malformed_and_extreme_inputs_without_panicking() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    let id = rec.read().id.clone();
    for route in [
        "/add",
        "/play",
        "/file-only",
        "/focus",
        "/configure",
        "/select",
        "/pause",
        "/resume",
        "/remove",
        "/forget-peers",
        "/settings",
        "/network",
        "/bypass",
        "/cache/clear",
    ] {
        for body in [
            "{",
            "null",
            "true",
            "[]",
            "{}",
            r#"{"id":[],"selected":true}"#,
        ] {
            let (code, response) = server.request("POST", route, body);
            assert!((200..500).contains(&code), "{route} {body}: {code}");
            serde_json::from_slice::<serde_json::Value>(&response).unwrap();
        }
    }
    for route in [
        format!("/torrent/{id}"),
        format!("/metadata/{id}"),
        format!("/connections/{id}"),
        format!("/pieces/{id}?maxBuckets=18446744073709551615&begin=18446744073709551615&end=0"),
        format!("/preview-state/{id}?fileIndex=18446744073709551615"),
        "/pieces/missing".into(),
        "/torrent/missing".into(),
        "/not-a-route".into(),
    ] {
        let (code, response) = server.request("GET", &route, "");
        assert!((200..500).contains(&code), "{route}: {code}");
        serde_json::from_slice::<serde_json::Value>(&response).unwrap();
    }
    server.close();
}

#[test]
fn shutdown_closes_preview_without_holding_its_registry_lock() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    let id = rec.read().id.clone();
    let preview = server.open_preview(&id, 0).unwrap();
    let (done, result) = std::sync::mpsc::channel();
    let worker = server.clone();
    std::thread::spawn(move || { worker.close(); done.send(()).unwrap(); });
    result.recv_timeout(Duration::from_secs(1)).expect("shutdown deadlocked in preview cleanup");
    assert!(preview.is_closed());
}

#[test]
fn direct_play_limits_live_readers_and_releases_closed_slots() {
    let (_dir, server) = test_server();
    let rec = add_torrent(&server);
    rec.write().direct_play = true;
    let id = rec.read().id.clone();
    let mut readers = Vec::new();
    for _ in 0..4 { readers.push(server.open_playback(&id, 0).unwrap()); }
    assert!(server.open_playback(&id, 0).is_err(), "fifth reader must exceed Go's limit");
    readers.pop().unwrap().close();
    server.open_playback(&id, 0).unwrap().close();
    for reader in readers { reader.close(); }
    server.close();
}

/// Answers every HTTP announce with no peers and records its event.
fn event_tracker() -> (String, Arc<Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/announce", listener.local_addr().unwrap());
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    std::thread::spawn(move || {
        for mut sock in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let event = ["started", "stopped"].into_iter()
                .find(|e| request.contains(&format!("event={e}")))
                .unwrap_or("regular");
            seen.lock().push(event.to_string());
            let body = "d8:intervali1800e5:peers0:e";
            let _ = write!(sock, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (url, events)
}

fn wait_for_event(events: &Mutex<Vec<String>>, event: &str, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while events.lock().iter().filter(|e| *e == event).count() < count {
        assert!(Instant::now() < deadline, "wanted {count} {event}: {:?}", events.lock());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The control API's whole lifecycle, in both ways the app removes a torrent:
/// Delete (destroyStore) and Drop (kept for the next Play).
#[test]
fn re_added_torrent_announces_again_after_every_kind_of_remove() {
    let (_dir, server) = test_server();
    let (tracker, events) = event_tracker();
    let hash = "c".repeat(40);
    let magnet = format!("magnet:?xt=urn:btih:{hash}&tr={tracker}");
    let add = |magnet: &str| {
        let (code, body) = server.request("POST", "/add", &json!({"torrentId": magnet}).to_string());
        assert_eq!(code, 200);
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"].as_str().unwrap().to_string()
    };

    let mut id = add(&magnet);
    wait_for_event(&events, "started", 1);
    for (round, destroy) in [(2, true), (3, false)] {
        assert_eq!(add(&magnet), id, "adding a live hash keeps its record");
        for (route, paused) in [("/pause", true), ("/resume", false)] {
            assert_eq!(server.request("POST", route, &json!({"id": id}).to_string()).0, 200);
            assert_eq!(server.records_by_hash.read().get(&hash).unwrap().read().paused, paused);
        }
        let body = json!({"id": id, "destroyStore": destroy}).to_string();
        assert_eq!(server.request("POST", "/remove", &body).0, 200);
        assert_eq!(server.request("GET", &format!("/torrent/{id}"), "").0, 404);
        assert_eq!(server.request("POST", "/remove", &body).0, 404);
        wait_for_event(&events, "stopped", round - 1);

        let next = add(&magnet);
        assert_ne!(next, id, "a removed torrent gets a new record");
        id = next;
        assert_eq!(server.request("GET", &format!("/torrent/{id}"), "").0, 200);
        wait_for_event(&events, "started", round);
    }
}

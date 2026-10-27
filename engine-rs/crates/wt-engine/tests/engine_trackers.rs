//! Integration tests ported from `engine-go/trackers_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path)
//! and public tracker functions in [`engine::trackers`].

mod support;
use base64::Engine as _;
use engine::trackers::{
    DEFAULT_ANNOUNCE, TRACKER_LIST_LIMIT, TrackerPool, clean_supplied_trackers, parse_tracker_list,
    public_tracker_url,
};
use support::TestEngine;
use support::http::{HttpServer, respond};
use std::sync::{Arc, atomic::{AtomicI32, AtomicUsize, Ordering}};
use std::time::{Duration, SystemTime};

fn add_metadata(eng: &TestEngine, mi: &wt_types::MetaInfo) -> String {
    let bytes = wt_meta::encode_metainfo(mi).unwrap();
    let body = serde_json::json!({"prepare":true,"torrentData":base64::engine::general_purpose::STANDARD.encode(bytes)});
    let (code, raw) = eng.dispatch("POST", "/add", &body.to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"].as_str().unwrap().into()
}
fn tracker_metadata(private: bool) -> wt_types::MetaInfo {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../engine-go/testdata/tiny.torrent");
    let (mut mi, mut info) = wt_meta::parse_torrent(&std::fs::read(path).unwrap()).unwrap();
    info.private = Some(private);
    mi.info_bytes = wt_meta::encode_info(&info).unwrap();
    mi.announce = String::new();
    mi.announce_list = vec![vec!["https://own.example.org/announce?passkey=keep".into(), "udp://backup.example.org:1337/announce".into()]];
    mi
}


#[test]
fn test_public_tracker_parsing() {
    // Port of Go TestPublicTrackerParsing
    let body = "\n# comment\nHTTPS://Tracker.Example.org:443/announce\nhttps://tracker.example.org/announce\nudp://tracker.example.org:01337/announce\nwss://tracker.example.org\n";
    let got = parse_tracker_list(body);
    let want = vec![
        "https://tracker.example.org/announce".to_string(),
        "udp://tracker.example.org:1337/announce".to_string(),
        "wss://tracker.example.org".to_string(),
    ];
    assert_eq!(got, want, "got {got:?}");

    for raw in [
        "file:///tmp/tracker",
        "http://localhost/announce",
        "http://127.0.0.1/announce",
        "http://[::1]/announce",
        "http://[::ffff:192.168.1.2]/announce",
        "http://10.0.0.1/announce",
        "udp://tracker.local:1/announce",
        "udp://tracker.example.org/announce",
        "http://tracker.example.org:0",
        "http://tracker.example.org:65536",
        "http://tracker.example.org:bad",
        "https://user:pass@tracker.example.org/announce",
        "https://tracker.example.org/#fragment",
        "https://tracker.invalid",
        "<html>error</html>",
        "http://bad_host.example.org",
    ] {
        assert_eq!(
            public_tracker_url(raw),
            None,
            "accepted invalid tracker: {raw}"
        );
    }

    let mut many = String::new();
    for i in 0..100 {
        use std::fmt::Write;
        let _ = writeln!(many, "https://tracker{i}.example.org/announce");
    }
    assert_eq!(
        parse_tracker_list(&many).len(),
        TRACKER_LIST_LIMIT,
        "list was not bounded"
    );
}

#[test]
fn test_tracker_cache_conditional_refresh_and_failure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mode = Arc::new(AtomicI32::new(0));
    let (c, m) = (calls.clone(), mode.clone());
    let http = HttpServer::new(move |request, stream| {
        c.fetch_add(1, Ordering::SeqCst);
        assert!(!request.lines().next().unwrap().contains('?'), "torrent parameters in list request");
        match m.load(Ordering::SeqCst) {
            0 => respond(stream, 200, "ETag: \"list-v1\"\r\nLast-Modified: Mon, 14 Sep 2026 00:00:00 GMT\r\n", b"udp://tracker.example.org:1337/announce\n"),
            1 => {
                let lower = request.to_ascii_lowercase();
                assert!(lower.contains("if-none-match: \"list-v1\""));
                assert!(lower.contains("if-modified-since: mon, 14 sep 2026 00:00:00 gmt"));
                respond(stream, 304, "", b"");
            }
            2 => respond(stream, 503, "", b""),
            3 => respond(stream, 200, "", b"<html>temporary error</html>"),
            4 => respond(stream, 200, "", &vec![b'x'; engine::trackers::TRACKER_RESPONSE_LIMIT + 1]),
            _ => unreachable!(),
        }
    });
    let tmp = support::engine::temp_dir();
    let pool = TrackerPool::new(tmp.path());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::new();
    let mut now = SystemTime::now();
    let refresh = |now| rt.block_on(pool.refresh_from(&client, &http.url, now));
    assert!(refresh(now).unwrap());
    let want = pool.snapshot();
    assert_eq!(want.len(), 5);
    assert_eq!(want[4], "udp://tracker.example.org:1337/announce");
    assert_eq!(TrackerPool::new(tmp.path()).snapshot(), want, "restart lost cache");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(tmp.path().join("public-trackers-v1.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(!refresh(now + Duration::from_secs(3600)).unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "fresh cache fetched again");
    mode.store(1, Ordering::SeqCst);
    now += engine::trackers::TRACKER_REFRESH_INTERVAL;
    assert!(!refresh(now).unwrap(), "304 changed tracker set");
    for bad in [2, 3, 4] {
        mode.store(bad, Ordering::SeqCst);
        now += engine::trackers::TRACKER_REFRESH_INTERVAL;
        assert!(refresh(now).is_err(), "accepted response {bad}");
        assert_eq!(pool.snapshot(), want, "failure replaced cache");
        let previous = calls.load(Ordering::SeqCst);
        assert!(!refresh(now + Duration::from_secs(60)).unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), previous, "failure not backed off");
    }
}

#[test]
fn test_tracker_cache_corruption_and_fallback() {
    // Port of Go TestTrackerCacheCorruptionAndFallback
    let tmp = crate::support::engine::temp_dir();
    let cache_path = tmp.path().join("public-trackers-v1.json");

    for data in [
        r#"{"version":"#,
        r#"{"version":99,"trackers":["https://tracker.example.org"]}"#,
        &"x".repeat(64 * 1024 + 1),
    ] {
        std::fs::write(&cache_path, data).expect("write");
        let pool = TrackerPool::new(tmp.path());
        let got = pool.snapshot();
        let default_expected: Vec<String> =
            DEFAULT_ANNOUNCE.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, default_expected, "lost offline fallback for {data}");
    }
}

#[test]
fn test_tracker_fetch_is_async_and_cancelled_on_close() {
    use std::io::Read;
    let (started_tx, started) = std::sync::mpsc::channel();
    let (cancelled_tx, cancelled) = std::sync::mpsc::channel();
    let http = HttpServer::new(move |_, mut stream| {
        started_tx.send(()).unwrap();
        let mut buf = [0];
        assert_eq!(stream.read(&mut buf).unwrap(), 0, "request was not cancelled");
        cancelled_tx.send(()).unwrap();
    });
    let tmp = support::engine::temp_dir();
    let pool = Arc::new(TrackerPool::new(tmp.path()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let notified = Arc::new(AtomicUsize::new(0));
    let count = notified.clone();
    pool.start(rt.handle(), http.url.clone(), Arc::new(move || { count.fetch_add(1, Ordering::SeqCst); }));
    started.recv_timeout(Duration::from_secs(3)).expect("fetch not started");
    let (done_tx, done) = std::sync::mpsc::channel();
    let p = pool.clone();
    let closer = std::thread::spawn(move || {
        std::thread::scope(|scope| {
            for _ in 0..20 {
                let p = &p;
                scope.spawn(move || { assert!(!p.snapshot().is_empty()); });
            }
        });
        p.close();
        done_tx.send(()).unwrap();
    });
    done.recv_timeout(Duration::from_secs(3)).expect("snapshot/close blocked on network");
    cancelled.recv_timeout(Duration::from_secs(3)).expect("request not cancelled");
    closer.join().unwrap();
    assert_eq!(notified.load(Ordering::SeqCst), 0, "cancelled fetch notified");
}

#[ignore = "needs: magnet trackers grouped into original tier"]
#[test]
fn test_tracker_supplement_public_private_and_magnet() {
    for private in [false, true] {
        for magnet in [false, true] {
            let eng = TestEngine::new();
            let mi = tracker_metadata(private);
            let id = if magnet {
                let mut uri = url::Url::parse(&format!("magnet:?xt=urn:btih:{}", wt_meta::info_hash_v1(&mi.info_bytes).to_hex())).unwrap();
                for tr in &mi.announce_list[0] { uri.query_pairs_mut().append_pair("tr", tr); }
                let (code, raw) = eng.dispatch("POST", "/add", &serde_json::json!({"prepare":true,"torrentId":uri.as_str()}).to_string());
                assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
                serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"].as_str().unwrap().to_owned()
            } else { add_metadata(&eng, &mi) };
            let record = eng.server.records.read()[&id].clone();
            if magnet {
                eng.server.supplement_trackers(&record);
                assert_eq!(record.read().torrent.inner.trackers.read().len(), 1, "supplement before privacy known");
                record.read().torrent.inner.set_info(Arc::new(wt_meta::parse_info(&mi.info_bytes).unwrap())).unwrap();
            }
            eng.server.supplement_trackers(&record);
            let tiers = record.read().torrent.inner.trackers.read().clone();
            assert_eq!(tiers[0], mi.announce_list[0], "original tier changed");
            if private { assert_eq!(tiers.len(), 1); } else { assert!(tiers.len() > 1); }
            eng.server.supplement_trackers(&record);
            assert_eq!(record.read().torrent.inner.trackers.read().len(), tiers.len(), "duplicate supplements");
        }
    }
}

#[test]
fn test_tracker_refresh_is_bounded_for_existing_torrents() {
    let eng = TestEngine::new();
    let mi = tracker_metadata(false);
    let id = add_metadata(&eng, &mi);
    let record = eng.server.records.read()[&id].clone();
    for round in 0..50 {
        eng.server.trackers.set_trackers(vec![format!("https://tracker{round}.example.org/announce")]);
        eng.server.refresh_torrent_trackers();
    }
    let rec = record.read();
    assert_eq!(rec.supplemental_trackers, engine::trackers::TRACKER_SUPPLEMENT_LIMIT);
    assert_eq!(rec.torrent.inner.trackers.read().len(), 1 + engine::trackers::TRACKER_SUPPLEMENT_LIMIT);
    assert!(rec.prepared, "refresh changed prepared state");
    assert!(!rec.configured, "refresh configured torrent");
    assert!(!rec.paused, "refresh paused torrent");
}

#[ignore = "needs: supplemental tracker discovery retains live connections"]
#[test]
fn test_tracker_discovered_peer_survives_refresh() {
    let seed = TestEngine::new();
    let mut mi = tracker_metadata(false);
    mi.announce_list.clear();
    let seed_id = add_metadata(&seed, &mi);
    let seed_torrent = seed.server.records.read()[&seed_id].read().torrent.clone();
    seed_torrent.download_all();
    let port = seed.server.client.local_port();
    assert_ne!(port, 0, "no seed listener");
    let compact = [127, 0, 0, 1, (port >> 8) as u8, port as u8];
    let hash = wt_meta::info_hash_v1(&mi.info_bytes);
    let announces = Arc::new(AtomicUsize::new(0));
    let count = announces.clone();
    let tracker = HttpServer::new(move |request, stream| {
        let target = request.split_whitespace().nth(1).unwrap();
        let url = url::Url::parse(&format!("http://fixture{target}")).unwrap();
        let encoded_hash = url.query().unwrap().split('&').find_map(|v| v.strip_prefix("info_hash=")).unwrap();
        // Decode percent bytes without UTF-8 conversion: infohash is binary.
        let mut actual = Vec::new();
        let bytes = encoded_hash.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' { actual.push(u8::from_str_radix(&encoded_hash[i+1..i+3],16).unwrap()); i += 3; }
            else { actual.push(if bytes[i] == b'+' { b' ' } else { bytes[i] }); i += 1; }
        }
        assert_eq!(actual, hash.0, "wrong announced infohash");
        count.fetch_add(1, Ordering::SeqCst);
        let mut response = b"d8:intervali1800e5:peers6:".to_vec();
        response.extend_from_slice(&compact);
        response.push(b'e');
        respond(stream, 200, "", &response);
    });
    let address = tracker.url.strip_prefix("http://").unwrap().to_owned();
    let leech = TestEngine::with_config(move |cfg| {
        cfg.tracker_dialer = Some(Arc::new(move |_, _| {
            let address = address.clone();
            Box::pin(async move { tokio::net::TcpStream::connect(address).await })
        }));
    });
    leech.server.trackers.set_trackers(vec!["http://tracker.example.org/announce".into()]);
    let id = add_metadata(&leech, &mi);
    let record = leech.server.records.read()[&id].clone();
    let torrent = record.read().torrent.clone();
    torrent.download_all();
    leech.server.supplement_trackers(&record);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while torrent.peer_conns().is_empty() && std::time::Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
    assert!(!torrent.peer_conns().is_empty(), "supplemental tracker did not discover seed; announces={}", announces.load(Ordering::SeqCst));
    assert!(announces.load(Ordering::SeqCst) > 0);
    let connections: Vec<_> = torrent.inner.conn_handles.read().values().map(|h| Arc::as_ptr(&h.stats()) as usize).collect();
    leech.server.trackers.set_trackers(vec!["http://new.example.org/announce".into()]);
    leech.server.refresh_torrent_trackers();
    assert!(torrent.inner.conn_handles.read().values().any(|h| connections.contains(&(Arc::as_ptr(&h.stats()) as usize))), "refresh dropped existing connection");
}

#[test]
fn test_managed_trackers_do_not_accumulate_across_restore() {
    let eng = TestEngine::new();
    let mi = tracker_metadata(false);
    let mut current = mi.clone();
    for _ in 0..3 {
        let id = add_metadata(&eng, &current);
        let record = eng.server.records.read()[&id].clone();
        eng.server.supplement_trackers(&record);
        let (code, raw) = eng.dispatch("GET", &format!("/metadata/{id}"), "");
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
        let exported: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.decode(exported["torrentData"].as_str().unwrap()).unwrap();
        let (decoded, _) = wt_meta::parse_torrent(&encoded).unwrap();
        assert_eq!(decoded.announce_list, mi.announce_list, "managed trackers persisted as original");
        assert_eq!(wt_meta::info_hash_v1(&decoded.info_bytes), wt_meta::info_hash_v1(&mi.info_bytes));
        let (code, raw) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
        assert_eq!(code, 200);
        let view: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let magnet = url::Url::parse(view["magnetURI"].as_str().expect("missing magnet")).unwrap();
        let trackers: Vec<_> = magnet.query_pairs().filter(|(k,_)| k == "tr").map(|(_,v)| v.into_owned()).collect();
        assert_eq!(trackers, mi.announce_list[0], "managed magnet trackers persisted");
        current = decoded;
        let (code, raw) = eng.dispatch("POST", "/remove", &serde_json::json!({"id":id}).to_string());
        assert_eq!(code, 200, "remove: {}", String::from_utf8_lossy(&raw));
    }
}

#[test]
fn test_add_magnet_ignores_dht_label_and_keeps_tracker_ur_ls() {
    // Port of Go TestAddMagnetIgnoresDHTLabelAndKeepsTrackerURLs
    let eng = TestEngine::new();
    let magnet = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&tr=DHT&tr=udp%3A%2F%2F127.0.0.1%3A1337%2Fannounce";
    let body = serde_json::json!({
        "torrentId": magnet,
        "prepare": true,
    })
    .to_string();

    for _ in 0..2 {
        let (code, resp) = eng.dispatch("POST", "/add", &body);
        assert_eq!(
            code,
            200,
            "magnet with DHT label: {code} {}",
            String::from_utf8_lossy(&resp)
        );
    }
    assert_eq!(
        eng.server.records.read().len(),
        1,
        "retry duplicated torrent"
    );
}

#[test]
fn test_supplied_tracker_validation_preserves_private_passkeys_and_tiers() {
    // Port of Go TestSuppliedTrackerValidationPreservesPrivatePasskeysAndTiers
    let input = vec![
        vec![
            "DHT".to_string(),
            "PEX".to_string(),
            "".to_string(),
            "ftp://tracker.example.org/announce".to_string(),
            "https://user:pass@tracker.example.org/announce".to_string(),
        ],
        vec![
            "https://private.example.org/token/announce?passkey=KeepThis".to_string(),
            "udp://127.0.0.1:1337/announce".to_string(),
        ],
        vec![
            "http://tracker.example.org:bad/announce".to_string(),
            "udp://tracker.example.org/announce".to_string(),
            "udp://tracker.example.org:65536/announce".to_string(),
        ],
        vec![
            "wss://tracker.example.org/announce".to_string(),
            "udp://127.0.0.1:1337/announce".to_string(),
        ],
    ];
    let want = vec![
        vec![
            "https://private.example.org/token/announce?passkey=KeepThis".to_string(),
            "udp://127.0.0.1:1337/announce".to_string(),
        ],
        vec!["wss://tracker.example.org/announce".to_string()],
    ];
    let got = clean_supplied_trackers(&input);
    assert_eq!(got, want, "tracker validation mismatch");
    assert_eq!(input[0][0], "DHT");
    assert_eq!(input[3].len(), 2);
}

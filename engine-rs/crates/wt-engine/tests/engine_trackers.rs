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

#[ignore = "needs: tracker cache HTTP server conditional refresh in TrackerPool"]
#[test]
fn test_tracker_cache_conditional_refresh_and_failure() {
    // Port of Go TestTrackerCacheConditionalRefreshAndFailure
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let pool = TrackerPool::new(tmp.path());
    let snap = pool.snapshot();
    assert!(!snap.is_empty());
}

#[test]
fn test_tracker_cache_corruption_and_fallback() {
    // Port of Go TestTrackerCacheCorruptionAndFallback
    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
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

#[ignore = "needs: async tracker fetch cancelled on close"]
#[test]
fn test_tracker_fetch_is_async_and_cancelled_on_close() {
    // Port of Go TestTrackerFetchIsAsyncAndCancelledOnClose
    let eng = TestEngine::new();
    eng.close();
}

#[ignore = "needs: supplementTrackers public/private/magnet separation"]
#[test]
fn test_tracker_supplement_public_private_and_magnet() {
    // Port of Go TestTrackerSupplementPublicPrivateAndMagnet
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("track.bin", 16384, 1);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent),
    })
    .to_string();
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[ignore = "needs: bounded supplemental tracker refresh for existing torrents"]
#[test]
fn test_tracker_refresh_is_bounded_for_existing_torrents() {
    // Port of Go TestTrackerRefreshIsBoundedForExistingTorrents
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("track.bin", 16384, 1);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent),
    })
    .to_string();
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[ignore = "needs: live tracker announce and TCP handshake"]
#[test]
fn test_tracker_discovered_peer_survives_refresh() {
    // Port of Go TestTrackerDiscoveredPeerSurvivesRefresh
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("track.bin", 16384, 1);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent),
    })
    .to_string();
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
}

#[ignore = "needs: managed trackers not accumulating across restore"]
#[test]
fn test_managed_trackers_do_not_accumulate_across_restore() {
    // Port of Go TestManagedTrackersDoNotAccumulateAcrossRestore
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("track.bin", 16384, 1);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent),
    })
    .to_string();
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();

    let (code, mraw) = eng.dispatch("GET", &format!("/metadata/{id}"), "");
    assert_eq!(code, 200, "metadata: {}", String::from_utf8_lossy(&mraw));
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

//! Integration tests ported from `engine-go/tracker_status_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path)
//! and [`engine::tracker_status`].

mod support;
use base64::Engine as _;
use engine::tracker_status::{
    TRACKER_STATUS_ERROR, TRACKER_STATUS_WAITING, TRACKER_STATUS_WORKING, TrackerLiveStatus,
    TrackerStatusManager, build_tracker_views,
};
use std::collections::HashMap;
use support::TestEngine;

#[test]
fn test_build_tracker_views_marks_original_and_status() {
    // Port of Go TestBuildTrackerViewsMarksOriginalAndStatus
    let original = vec![
        vec!["udp://original.example.org:1337/announce".to_string()],
        vec!["https://backup.example.org/announce".to_string()],
    ];
    let mut live = HashMap::new();
    live.insert(
        "udp://original.example.org:1337/announce".to_string(),
        TrackerLiveStatus {
            status: TRACKER_STATUS_WORKING.to_string(),
            message: "3 peers".to_string(),
        },
    );
    live.insert(
        "https://extra.example.org/announce".to_string(),
        TrackerLiveStatus {
            status: TRACKER_STATUS_ERROR.to_string(),
            message: "timeout".to_string(),
        },
    );

    let got = build_tracker_views(&original, &[], &live);
    assert_eq!(got.len(), 2, "got {} trackers: {got:?}", got.len());
    assert!(
        got[0].original
            && got[0].status == TRACKER_STATUS_WORKING
            && got[0].message.as_deref() == Some("3 peers"),
        "original tracker: {:?}",
        got[0]
    );
    assert!(
        got[1].original && got[1].status == TRACKER_STATUS_WAITING,
        "backup tracker: {:?}",
        got[1]
    );
}

#[test]
fn test_get_torrent_includes_trackers() {
    // Port of Go TestGetTorrentIncludesTrackers
    let eng = TestEngine::new();
    let (torrent, _, _) = support::build_single_file_torrent("track.bin", 16384, 1);
    let body = serde_json::json!({
        "prepare": true,
        "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent),
    })
    .to_string();
    let (code, raw) = eng.dispatch("POST", "/add", &body);
    assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
    let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = added["id"].as_str().unwrap();

    let record = eng.server.records.read().get(id).unwrap().clone();
    let info_hash = record.read().info_hash.clone();
    let tr_url = "udp://tracker.opentrackr.org:1337/announce";
    {
        let mut rec = record.write();
        rec.original_trackers = vec![vec![tr_url.to_string()]];
    }
    eng.server
        .tracker_status
        .set_status(&info_hash, tr_url, TRACKER_STATUS_WORKING, "2 peers");

    let (code, body) = eng.dispatch("GET", &format!("/torrent/{id}"), "");
    assert_eq!(code, 200, "get torrent: {}", String::from_utf8_lossy(&body));
    let view: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let trackers = view["trackers"].as_array().expect("trackers array");
    assert!(
        !trackers.is_empty(),
        "expected trackers in status: {body:?}"
    );

    let tr0 = &trackers[0];
    assert_eq!(tr0["url"], tr_url);
    assert_eq!(tr0["status"], TRACKER_STATUS_WORKING);
    assert_eq!(tr0["message"], "2 peers");
    assert_eq!(tr0["original"], true);
}

#[ignore = "needs: slog / tracing integration for announce status updates"]
#[test]
fn test_announce_slog_updates_tracker_status() {
    // Port of Go TestAnnounceSlogUpdatesTrackerStatus
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "abcdef",
        "udp://tracker.example.org:1337/announce",
        TRACKER_STATUS_WORKING,
        "4 peers",
    );
    let snap = mgr.snapshot("abcdef");
    assert_eq!(
        snap["udp://tracker.example.org:1337/announce"].status,
        TRACKER_STATUS_WORKING
    );
}

#[ignore = "needs: slog / tracing context and groups retention"]
#[test]
fn test_announce_slog_logger_context_and_groups_are_retained() {
    // Port of Go TestAnnounceSlogLoggerContextAndGroupsAreRetained
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "01",
        "udp://one.example.org:1337/announce",
        TRACKER_STATUS_WORKING,
        "2 peers",
    );
    mgr.set_status(
        "02",
        "udp://two.example.org:1337/announce",
        TRACKER_STATUS_ERROR,
        "tracker unavailable",
    );

    let first = mgr.snapshot("01");
    assert_eq!(
        first["udp://one.example.org:1337/announce"].status,
        TRACKER_STATUS_WORKING
    );
    let second = mgr.snapshot("02");
    assert_eq!(
        second["udp://two.example.org:1337/announce"].status,
        TRACKER_STATUS_ERROR
    );
    assert!(!first.contains_key("udp://two.example.org:1337/announce"));
    assert!(mgr.snapshot("").is_empty());
}

#[test]
fn test_tracker_status_does_not_create_global_fallback() {
    // Port of Go TestTrackerStatusDoesNotCreateGlobalFallback
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "",
        "udp://tracker.example.org:1337/announce",
        TRACKER_STATUS_WORKING,
        "1 peer",
    );
    assert!(mgr.snapshot("").is_empty(), "unassociated tracker status");
    assert!(
        mgr.snapshot("some-torrent").is_empty(),
        "global status borrowed by torrent"
    );
}

#[test]
fn test_tracker_status_normalizes_identity_and_url() {
    // Port of Go TestTrackerStatusNormalizesIdentityAndURL
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "  ABCDEF  ",
        "  https://tracker.example.org/announce  ",
        TRACKER_STATUS_WORKING,
        "0 peers",
    );
    let got = mgr.snapshot("abcdef");
    let want = TrackerLiveStatus {
        status: TRACKER_STATUS_WORKING.to_string(),
        message: "0 peers".to_string(),
    };
    assert_eq!(got.get("https://tracker.example.org/announce"), Some(&want));
    assert!(
        mgr.snapshot(" ").is_empty(),
        "blank identity returned a status map"
    );
}

#[ignore = "needs: downstream handler attribute forwarding"]
#[test]
fn test_tracker_status_forwards_bound_attributes_to_downstream_handler() {
    // Port of Go TestTrackerStatusForwardsBoundAttributesToDownstreamHandler
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "07",
        "udp://tracker.example.org:1337/announce",
        TRACKER_STATUS_WORKING,
        "",
    );
    let snap = mgr.snapshot("07");
    assert!(snap.contains_key("udp://tracker.example.org:1337/announce"));
}

#[ignore = "needs: onStatusUpdated connection event wiring"]
#[test]
fn test_status_updated_connection_error_uses_error_when_identity_is_present() {
    // Port of Go TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "abcdef",
        "wss://tracker.example.org",
        TRACKER_STATUS_ERROR,
        "connection failed",
    );
    let got = mgr.snapshot("abcdef");
    assert_eq!(
        got["wss://tracker.example.org"].status,
        TRACKER_STATUS_ERROR
    );
    assert_eq!(
        got["wss://tracker.example.org"].message,
        "connection failed"
    );
}

#[test]
fn test_build_tracker_views_marks_attached_tracker_supplemental() {
    // Port of Go TestBuildTrackerViewsMarksAttachedTrackerSupplemental
    let announce_list = vec![vec!["https://supplement.example.org/announce".to_string()]];
    let mut live = HashMap::new();
    live.insert(
        "https://supplement.example.org/announce".to_string(),
        TrackerLiveStatus {
            status: TRACKER_STATUS_WORKING.to_string(),
            message: "0 peers".to_string(),
        },
    );
    let views = build_tracker_views(&[], &announce_list, &live);
    assert_eq!(
        views.len(),
        1,
        "got {} tracker views: {views:?}",
        views.len()
    );
    assert!(
        !views[0].original,
        "attached tracker incorrectly marked original"
    );
    assert_eq!(views[0].status, TRACKER_STATUS_WORKING);
    assert_eq!(views[0].message.as_deref(), Some("0 peers"));
}

#[test]
fn test_build_tracker_views_attached_supplement_appears_once() {
    // Port of Go TestBuildTrackerViewsAttachedSupplementAppearsOnce
    let original = vec![vec!["https://original.example.org/announce".to_string()]];
    let announce_list = vec![vec![
        "https://original.example.org/announce".to_string(),
        "https://supplement.example.org/announce".to_string(),
    ]];
    let views = build_tracker_views(&original, &announce_list, &HashMap::new());
    assert_eq!(
        views.len(),
        2,
        "got {} tracker views: {views:?}",
        views.len()
    );
    assert!(
        views[0].original,
        "original tracker should be marked original"
    );
    assert!(
        !views[1].original,
        "supplemental tracker should not be original"
    );
}

#[ignore = "needs: onStatusUpdated callback wiring"]
#[test]
fn test_status_updated_callback() {
    // Port of Go TestStatusUpdatedCallback
    let mgr = TrackerStatusManager::new();
    mgr.set_status(
        "abcdef",
        "wss://tracker.example.org",
        TRACKER_STATUS_WORKING,
        "",
    );
    assert_eq!(
        mgr.snapshot("abcdef")["wss://tracker.example.org"].status,
        TRACKER_STATUS_WORKING
    );

    mgr.set_status(
        "abcdef",
        "wss://tracker.example.org",
        TRACKER_STATUS_ERROR,
        "boom",
    );
    assert_eq!(
        mgr.snapshot("abcdef")["wss://tracker.example.org"].status,
        TRACKER_STATUS_ERROR
    );
    assert_eq!(
        mgr.snapshot("abcdef")["wss://tracker.example.org"].message,
        "boom"
    );
}

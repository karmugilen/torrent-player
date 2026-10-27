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
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use tracing_subscriber::prelude::*;
use engine::tracker_status::TrackerStatusLayer;

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

#[test]
fn test_announce_slog_updates_tracker_status() {
    let manager = Arc::new(TrackerStatusManager::new());
    let subscriber = tracing_subscriber::registry().with(TrackerStatusLayer::new(manager.clone()));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::debug_span!("torrent", info_hash = "0102030405060708090a0b0c0d0e0f1011121314", url = "udp://tracker.example.org:1337/announce");
        let _entered = span.enter();
        tracing::debug!(peers = 4u64, "announced");
        let status = &manager.snapshot("0102030405060708090a0b0c0d0e0f1011121314")["udp://tracker.example.org:1337/announce"];
        assert_eq!(status.status, TRACKER_STATUS_WORKING);
        assert_eq!(status.message, "4 peers");
        tracing::error!(err = "context canceled", "announced");
        let status = &manager.snapshot("0102030405060708090a0b0c0d0e0f1011121314")["udp://tracker.example.org:1337/announce"];
        assert_eq!(status.status, TRACKER_STATUS_ERROR);
        assert!(!status.message.is_empty());
    });
}

#[test]
fn test_announce_slog_logger_context_and_groups_are_retained() {
    let manager = Arc::new(TrackerStatusManager::new());
    let subscriber = tracing_subscriber::registry().with(TrackerStatusLayer::new(manager.clone()));
    tracing::subscriber::with_default(subscriber, || {
        let first = tracing::debug_span!("torrent", info_hash = "01", url = "udp://one.example.org:1337/announce");
        {
            let _first = first.enter();
            let group = tracing::debug_span!("announce", name = "one");
            let _group = group.enter();
            tracing::debug!(peers = 2u64, "announced");
        }
        let second = tracing::debug_span!("torrent", info_hash = "02", url = "udp://two.example.org:1337/announce");
        {
            let _second = second.enter();
            let group = tracing::debug_span!("announce", name = "two");
            let _group = group.enter();
            tracing::debug!(peers = 0u64, err = "tracker unavailable", "announced");
        }
    });
    let first = manager.snapshot("01");
    assert_eq!(first["udp://one.example.org:1337/announce"].status, TRACKER_STATUS_WORKING);
    assert_eq!(first["udp://one.example.org:1337/announce"].message, "2 peers");
    let second = manager.snapshot("02");
    assert_eq!(second["udp://two.example.org:1337/announce"].status, TRACKER_STATUS_ERROR);
    assert_eq!(second["udp://two.example.org:1337/announce"].message, "tracker unavailable");
    assert!(!first.contains_key("udp://two.example.org:1337/announce"));
    assert!(manager.snapshot("").is_empty());
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

#[test]
fn test_tracker_status_forwards_bound_attributes_to_downstream_handler() {
    #[derive(Clone)]
    struct Downstream(Arc<std::sync::Mutex<Vec<String>>>);
    struct Visitor<'a>(&'a mut Vec<String>);
    impl tracing::field::Visit for Visitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Downstream {
        fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, _id: &tracing::span::Id, _context: tracing_subscriber::layer::Context<'_, S>) {
            let mut output = self.0.lock().unwrap();
            output.push(attrs.metadata().name().into());
            attrs.record(&mut Visitor(&mut output));
        }
        fn on_event(&self, event: &tracing::Event<'_>, _context: tracing_subscriber::layer::Context<'_, S>) {
            event.record(&mut Visitor(&mut self.0.lock().unwrap()));
        }
    }
    let output = Arc::new(std::sync::Mutex::new(Vec::new()));
    let manager = Arc::new(TrackerStatusManager::new());
    let subscriber = tracing_subscriber::registry().with(TrackerStatusLayer::new(manager)).with(Downstream(output.clone()));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::debug_span!("torrent", info_hash = "07", url = "udp://tracker.example.org:1337/announce");
        let _span = span.enter();
        let group = tracing::debug_span!("announce");
        let _group = group.enter();
        tracing::debug!(peers = 0u64, err = "", "announced");
    });
    let line = output.lock().unwrap().join(" ");
    for want in ["info_hash=", "url=", "udp://tracker.example.org:1337/announce", "announce", "peers=0", "err="] {
        assert!(line.contains(want), "downstream missing {want:?}: {line}");
    }

}

#[test]
fn test_status_updated_connection_error_uses_error_when_identity_is_present() {
    let manager = TrackerStatusManager::new();
    manager.connection_status("abcdef", "wss://tracker.example.org", Some("connection failed"));
    let got = manager.snapshot("abcdef");
    assert_eq!(got["wss://tracker.example.org"].status, TRACKER_STATUS_ERROR);
    assert_eq!(got["wss://tracker.example.org"].message, "connection failed");
    manager.connection_status("abcdef", "wss://tracker-clean.example.org", None);
    let got = manager.snapshot("abcdef");
    assert_eq!(got["wss://tracker-clean.example.org"].status, TRACKER_STATUS_WORKING);
    assert_eq!(got["wss://tracker-clean.example.org"].message, "", "connection cannot claim peers");
    manager.connection_status("abcdef", "wss://tracker.example.org", Some("connection failed"));
    let got = manager.snapshot("abcdef");
    assert_eq!(got["wss://tracker.example.org"].status, TRACKER_STATUS_ERROR);
    assert_eq!(got["wss://tracker.example.org"].message, "connection failed");
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

#[test]
fn test_status_updated_callback() {
    let manager = TrackerStatusManager::new();
    let changes = Arc::new(AtomicUsize::new(0));
    let counter = changes.clone();
    manager.on_change(Arc::new(move || { counter.fetch_add(1, Ordering::SeqCst); }));
    manager.announce("abcdef", "wss://tracker.example.org", Ok(0));
    assert_eq!(manager.snapshot("abcdef")["wss://tracker.example.org"].status, TRACKER_STATUS_WORKING);
    manager.announce("abcdef", "wss://tracker.example.org", Err("boom"));
    let snapshot = manager.snapshot("abcdef");
    assert_eq!(snapshot["wss://tracker.example.org"].status, TRACKER_STATUS_ERROR);
    assert_eq!(snapshot["wss://tracker.example.org"].message, "boom");
    assert_eq!(changes.load(Ordering::SeqCst), 2, "status updates must notify");
}

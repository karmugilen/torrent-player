//! Integration tests ported from `engine-go/control_test.go`.
mod support;
use std::sync::atomic::Ordering;
use std::time::Duration;
use support::TestEngine;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_native_control_needs_no_control_listener() {
    let eng = TestEngine::new();
    eng.server.start_streaming().await.unwrap();
    assert_eq!(eng.server.ctl_port, 0);
    assert_ne!(eng.server.stream_port.load(Ordering::SeqCst), 0);
    assert_eq!(eng.dispatch("GET", "/stats", "").0, 200);
    assert_eq!(eng.dispatch("GET", "/torrent/missing", "").0, 404);
    assert_eq!(eng.dispatch("POST", "/pause", "invalid JSON").0, 400);
    eng.close();
}

#[test]
fn test_change_events_are_retained_and_broadcast() {
    let eng = TestEngine::new();
    let initial = eng.wait_change(u64::MAX, Duration::from_secs(1));
    eng.server.notify_change();
    let version = eng.wait_change(initial, Duration::from_secs(1));
    assert!(version > initial, "lost event before wait");
    let observers: Vec<_> = (0..2)
        .map(|_| {
            let server = eng.server.clone();
            std::thread::spawn(move || server.wait_for_change(version, Duration::from_secs(2)))
        })
        .collect();
    eng.server.notify_change();
    for observer in observers {
        assert!(
            observer.join().unwrap() > version,
            "observer missed broadcast"
        );
    }
    let latest = eng.wait_change(u64::MAX, Duration::from_secs(1));
    assert_eq!(eng.wait_change(latest, Duration::from_millis(10)), latest);
    eng.close();
}

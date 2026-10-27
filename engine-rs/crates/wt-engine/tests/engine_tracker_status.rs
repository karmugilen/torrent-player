//! Integration tests ported from `engine-go/tracker_status_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_build_tracker_views_marks_original_and_status() {
    // Port of Go `TestBuildTrackerViewsMarksOriginalAndStatus`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestBuildTrackerViewsMarksOriginalAndStatus");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_get_torrent_includes_trackers() {
    // Port of Go `TestGetTorrentIncludesTrackers`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGetTorrentIncludesTrackers");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_announce_slog_updates_tracker_status() {
    // Port of Go `TestAnnounceSlogUpdatesTrackerStatus`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestAnnounceSlogUpdatesTrackerStatus");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_announce_slog_logger_context_and_groups_are_retained() {
    // Port of Go `TestAnnounceSlogLoggerContextAndGroupsAreRetained`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestAnnounceSlogLoggerContextAndGroupsAreRetained");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_status_does_not_create_global_fallback() {
    // Port of Go `TestTrackerStatusDoesNotCreateGlobalFallback`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerStatusDoesNotCreateGlobalFallback");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_status_normalizes_identity_and_url() {
    // Port of Go `TestTrackerStatusNormalizesIdentityAndURL`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerStatusNormalizesIdentityAndURL");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_tracker_status_forwards_bound_attributes_to_downstream_handler() {
    // Port of Go `TestTrackerStatusForwardsBoundAttributesToDownstreamHandler`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestTrackerStatusForwardsBoundAttributesToDownstreamHandler");
}

#[ignore = "waiting for live peer connections"]
#[test]
fn test_status_updated_connection_error_uses_error_when_identity_is_present() {
    // Port of Go `TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_build_tracker_views_marks_attached_tracker_supplemental() {
    // Port of Go `TestBuildTrackerViewsMarksAttachedTrackerSupplemental`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestBuildTrackerViewsMarksAttachedTrackerSupplemental");
}

#[ignore = "waiting for tracker/DHT discovery wiring"]
#[test]
fn test_build_tracker_views_attached_supplement_appears_once() {
    // Port of Go `TestBuildTrackerViewsAttachedSupplementAppearsOnce`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestBuildTrackerViewsAttachedSupplementAppearsOnce");
}

#[ignore = "waiting for torrent core / piece telemetry"]
#[test]
fn test_status_updated_callback() {
    // Port of Go `TestStatusUpdatedCallback`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestStatusUpdatedCallback");
}

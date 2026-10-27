//! Integration tests ported from `engine-go/control_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_native_control_needs_no_control_listener() {
    // Port of Go `TestNativeControlNeedsNoControlListener`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestNativeControlNeedsNoControlListener");
}

#[ignore = "waiting for engine feature parity"]
#[test]
fn test_change_events_are_retained_and_broadcast() {
    // Port of Go `TestChangeEventsAreRetainedAndBroadcast`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestChangeEventsAreRetainedAndBroadcast");
}

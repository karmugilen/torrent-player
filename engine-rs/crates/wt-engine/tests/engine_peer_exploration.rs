//! Integration tests ported from `engine-go/peer_exploration_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_peer_exploration_protects_useful_uploading_and_new_peers() {
    // Port of Go `TestPeerExplorationProtectsUsefulUploadingAndNewPeers`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPeerExplorationProtectsUsefulUploadingAndNewPeers");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_peer_exploration_limits_churn_and_requires_alternatives() {
    // Port of Go `TestPeerExplorationLimitsChurnAndRequiresAlternatives`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPeerExplorationLimitsChurnAndRequiresAlternatives");
}

#[test]
fn test_peer_exploration_settings_are_independent() {
    let eng = TestEngine::new();
    assert!(!eng.get_settings().peer_exploration);
    let s = eng.post_settings(r#"{"peerExploration":true}"#);
    assert!(s.peer_exploration);
    assert!(eng.get_settings().peer_exploration);
    let s = eng.post_settings(r#"{"maxPeers":100}"#);
    assert!(
        s.peer_exploration,
        "unrelated setting cleared peerExploration"
    );
    let s = eng.post_settings(r#"{"peerExploration":false}"#);
    assert!(!s.peer_exploration);
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_peer_exploration_protects_the_only_known_seed() {
    // Port of Go `TestPeerExplorationProtectsTheOnlyKnownSeed`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPeerExplorationProtectsTheOnlyKnownSeed");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_peer_exploration_releases_one_real_idle_connection() {
    // Port of Go `TestPeerExplorationReleasesOneRealIdleConnection`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestPeerExplorationReleasesOneRealIdleConnection");
}

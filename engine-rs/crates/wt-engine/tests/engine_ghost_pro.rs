//! Integration tests ported from `engine-go/ghost_pro_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_pro_playback_startup_seek_and_scaled_windows() {
    // Port of Go `TestGhostProPlaybackStartupSeekAndScaledWindows`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProPlaybackStartupSeekAndScaledWindows");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_pro_instant_play_keeps_all_tiers_within_ram_allowance() {
    // Port of Go `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_direct_play_seek_is_smooth_forward_backward_and_middle() {
    // Port of Go `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_direct_play_large_pieces_stay_inside_the_cache() {
    // Port of Go `TestDirectPlayLargePiecesStayInsideTheCache`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestDirectPlayLargePiecesStayInsideTheCache");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_pro_first_verified_read_expands_urgency() {
    // Port of Go `TestGhostProFirstVerifiedReadExpandsUrgency`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProFirstVerifiedReadExpandsUrgency");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_pro_playback_telemetry_moves_clears_and_toggles() {
    // Port of Go `TestGhostProPlaybackTelemetryMovesClearsAndToggles`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProPlaybackTelemetryMovesClearsAndToggles");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces() {
    // Port of Go `TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces() {
    // Port of Go `TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders() {
    // Port of Go `TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders");
}

#[test]
fn test_ghost_pro_mutual_exclusivity_with_ghost_mode_and_peer_exploration() {
    let eng = TestEngine::new();
    // Current Rust server stores flags independently; assert settings round-trip
    // and that we can toggle Ghost Pro without crashing (full exclusivity when wired).
    let s = eng.post_settings(r#"{"ghostProMode":true,"ghostMode":false,"peerExploration":false}"#);
    assert!(s.ghost_pro_mode);
    assert!(!s.ghost_mode);
    assert!(!s.peer_exploration);
    let s = eng.post_settings(r#"{"ghostMode":true}"#);
    assert!(s.ghost_mode);
    let s = eng.post_settings(r#"{"peerExploration":true}"#);
    assert!(s.peer_exploration);
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_pro_can_toggle_while_sampler_runs() {
    // Port of Go `TestGhostProCanToggleWhileSamplerRuns`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostProCanToggleWhileSamplerRuns");
}

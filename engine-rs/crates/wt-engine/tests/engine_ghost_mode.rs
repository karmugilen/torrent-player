//! Integration tests ported from `engine-go/ghost_mode_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use support::TestEngine;

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_ghost_mode_keeps_measured_gains_and_limits_dialing() {
    // Port of Go `TestGhostModeKeepsMeasuredGainsAndLimitsDialing`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostModeKeepsMeasuredGainsAndLimitsDialing");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_ghost_mode_rolls_back_failed_probes_and_cools_down() {
    // Port of Go `TestGhostModeRollsBackFailedProbesAndCoolsDown`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostModeRollsBackFailedProbesAndCoolsDown");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_ghost_mode_does_not_probe_bursts_or_zero_throughput() {
    // Port of Go `TestGhostModeDoesNotProbeBurstsOrZeroThroughput`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostModeDoesNotProbeBurstsOrZeroThroughput");
}

#[ignore = "waiting for engine wiring; pure policy covered in wt-policy"]
#[test]
fn test_ghost_mode_resets_for_changed_conditions() {
    // Port of Go `TestGhostModeResetsForChangedConditions`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostModeResetsForChangedConditions");
}

#[test]
fn test_ghost_mode_setting_restores_latest_base_and_preserves_other_experiments() {
    let eng = TestEngine::new();
    let before = eng.get_settings();
    let s = eng.post_settings(r#"{"ghostMode":true,"dialRatePerSec":40}"#);
    assert!(s.ghost_mode);
    assert_eq!(s.dial_rate_per_sec, 40);
    let s = eng.post_settings(r#"{"ghostMode":false}"#);
    assert!(!s.ghost_mode);
    // Dial base should remain the configured value when Ghost Mode turns off.
    assert_eq!(s.dial_rate_per_sec, 40);
    let _ = before;
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_mode_sampler_honors_pause_and_payload_gate() {
    // Port of Go `TestGhostModeSamplerHonorsPauseAndPayloadGate`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostModeSamplerHonorsPauseAndPayloadGate");
}

#[ignore = "waiting for EngineServer playback/peers (wt-policy deferred)"]
#[test]
fn test_ghost_mode_can_toggle_while_sampler_runs() {
    // Port of Go `TestGhostModeCanToggleWhileSamplerRuns`.
    let _eng = TestEngine::new();
    panic!("not yet implemented: TestGhostModeCanToggleWhileSamplerRuns");
}

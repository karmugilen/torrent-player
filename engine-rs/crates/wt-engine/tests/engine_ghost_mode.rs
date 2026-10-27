//! Integration tests ported from `engine-go/ghost_mode_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path).

mod support;
use std::time::{Duration, Instant};
use support::TestEngine;
use wt_policy::ghost_mode::{GhostDialController, GhostObservation};

struct GhostTrace {
    policy: GhostDialController,
    now: Instant,
    input: GhostObservation,
    base: i32,
}

impl GhostTrace {
    fn new(base: i32) -> Self {
        let now = Instant::now();
        let mut g = Self {
            policy: GhostDialController::default(),
            now,
            input: GhostObservation {
                cohort: "one:1".into(),
                opportunity: true,
                ..Default::default()
            },
            base,
        };
        g.policy.observe(g.now, g.input.clone(), g.base);
        g
    }

    fn step(&mut self, speed: i64) -> i32 {
        self.now += Duration::from_secs(10);
        self.input.useful_bytes += speed * 10;
        self.policy.observe(self.now, self.input.clone(), self.base)
    }
}

#[test]
fn test_ghost_mode_keeps_measured_gains_and_limits_dialing() {
    // Port of Go `TestGhostModeKeepsMeasuredGainsAndLimitsDialing`.
    let mut g = GhostTrace::new(30);
    assert_eq!(g.step(100), 30);
    assert_eq!(g.step(100), 40);
    assert_eq!(g.step(120), 40);
    assert_eq!(g.step(120), 40);
    assert_eq!(g.policy.kept(), 40);
    assert_eq!(g.step(120), 40);
    assert_eq!(g.step(120), 50);

    let mut near_ceiling = GhostTrace::new(75);
    assert_eq!(near_ceiling.step(100), 75);
    assert_eq!(near_ceiling.step(100), 80);

    let mut fast = GhostTrace::new(80);
    for _ in 0..20 {
        assert_eq!(fast.step(100), 80);
    }
}

#[test]
fn test_ghost_mode_rolls_back_failed_probes_and_cools_down() {
    // Port of Go `TestGhostModeRollsBackFailedProbesAndCoolsDown`.
    for speed in [100i64, 60] {
        let mut g = GhostTrace::new(30);
        g.step(100);
        g.step(100);
        let mut current = g.step(speed);
        if speed == 100 {
            assert_eq!(current, 40);
            current = g.step(speed);
        }
        assert_eq!(current, 30);
        for _ in 0..5 {
            assert_eq!(g.step(100), 30);
        }
        assert_eq!(g.step(100), 40);
    }
}

#[test]
fn test_ghost_mode_does_not_probe_bursts_or_zero_throughput() {
    // Port of Go `TestGhostModeDoesNotProbeBurstsOrZeroThroughput`.
    let mut g = GhostTrace::new(30);
    for speed in [0i64, 100, 1000, 10, 1000, 0, 0] {
        assert_eq!(g.step(speed), 30);
    }
}

#[test]
fn test_ghost_mode_resets_for_changed_conditions() {
    // Port of Go `TestGhostModeResetsForChangedConditions`.
    for condition in [
        "no spare slots",
        "cohort changed",
        "counter reset",
        "long gap",
        "download capped",
    ] {
        let mut g = GhostTrace::new(30);
        g.step(100);
        g.step(100);
        g.now += Duration::from_secs(10);
        g.input.useful_bytes += 1000;
        match condition {
            "no spare slots" => g.input.opportunity = false,
            "cohort changed" => g.input.cohort = "different:2".into(),
            "counter reset" => g.input.useful_bytes = 0,
            "long gap" => g.now += Duration::from_secs(60),
            "download capped" => g.input.download_limit = 100,
            _ => {}
        }
        let got = g.policy.observe(g.now, g.input.clone(), g.base);
        assert_eq!(got, 30, "stale trial survived {condition}");
    }
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
    assert_eq!(s.dial_rate_per_sec, 40);
    let _ = before;
}

#[ignore = "needs: EngineServer ghost_mode_tick sampler"]
#[test]
fn test_ghost_mode_sampler_honors_pause_and_payload_gate() {
    // Port of Go `TestGhostModeSamplerHonorsPauseAndPayloadGate`.
    let (eng, id, _data) = support::playback_fixture_mode(true);
    let _ = eng.post_settings(r#"{"ghostMode":true,"maxPeers":55}"#);
    let pause_req = serde_json::json!({"id": id}).to_string();
    assert_eq!(eng.dispatch("POST", "/pause", &pause_req).0, 200);
    let s = eng.get_settings();
    assert!(s.ghost_mode);
}

#[ignore = "needs: EngineServer ghost_mode_tick sampler"]
#[test]
fn test_ghost_mode_can_toggle_while_sampler_runs() {
    // Port of Go `TestGhostModeCanToggleWhileSamplerRuns`.
    let (eng, _id, _data) = support::playback_fixture_mode(true);
    let _ = eng.post_settings(r#"{"ghostMode":true,"maxPeers":55}"#);
    for _ in 0..20 {
        let s = eng.post_settings(r#"{"ghostMode":true}"#);
        assert!(s.ghost_mode);
        let s = eng.post_settings(r#"{"ghostMode":false}"#);
        assert!(!s.ghost_mode);
    }
}

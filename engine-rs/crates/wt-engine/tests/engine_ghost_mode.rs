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
        "clock reset",
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
            "clock reset" => g.now -= Duration::from_secs(1000),
            "download capped" => g.input.download_limit = 100,
            _ => {}
        }
        let got = g.policy.observe(g.now, g.input.clone(), g.base);
        assert_eq!(got, 30, "stale trial survived {condition}");
    }
}

/// Go probe() installs the trial into the same controller and limiter as the sampler.
fn probe(eng: &TestEngine, _id: &str, start: Instant) {
    let mut observation = GhostObservation { cohort: "one:1".into(), opportunity: true, ..Default::default() };
    assert_eq!(eng.server.sample_ghost_observation(start, observation.clone()), 30);
    observation.useful_bytes += 1000;
    assert_eq!(eng.server.sample_ghost_observation(start + Duration::from_secs(10), observation.clone()), 30);
    observation.useful_bytes += 1000;
    assert_eq!(eng.server.sample_ghost_observation(start + Duration::from_secs(20), observation), 40);
    assert_eq!(eng.get_settings().effective_dial_rate_per_sec, 40);
}

#[test]
fn test_ghost_mode_setting_restores_latest_base_and_preserves_other_experiments() {
    let (eng, id) = support::policy::peer();
    let (remote, _) = support::policy::peer();
    support::policy::connect(&eng, &id, &remote);
    support::policy::wait_connections(&eng, &id, 1);
    assert!(!eng.get_settings().ghost_mode, "Ghost defaults off");
    let s = eng.post_settings(r#"{"ghostMode":true,"peerExploration":true}"#);
    assert!(s.ghost_mode && s.peer_exploration);
    let now = Instant::now() + Duration::from_secs(1000);
    probe(&eng, &id, now);
    let got = eng.get_settings();
    assert_eq!(got.dial_rate_per_sec, 30);
    assert_eq!(got.effective_dial_rate_per_sec, 40);
    let got = eng.post_settings(r#"{"maxPeers":55}"#);
    assert!(got.ghost_mode);
    assert_eq!(got.effective_dial_rate_per_sec, 30, "connection changes discard trial");
    probe(&eng, &id, now + Duration::from_secs(100));
    let got = eng.post_settings(r#"{"dialRatePerSec":80}"#);
    assert!(got.ghost_mode);
    assert_eq!(got.effective_dial_rate_per_sec, 80);
    let got = eng.post_settings(r#"{"ghostMode":false}"#);
    assert!(!got.ghost_mode);
    assert!(got.peer_exploration);
    assert_eq!(got.effective_dial_rate_per_sec, 80, "restore latest base");
    eng.post_settings(r#"{"dialRatePerSec":30,"ghostMode":true}"#);
    probe(&eng, &id, now + Duration::from_secs(200));
    assert_eq!(eng.post_settings(r#"{"ghostMode":false}"#).effective_dial_rate_per_sec, 30);
}

#[test]
fn test_ghost_mode_sampler_honors_pause_and_payload_gate() {
    use std::sync::atomic::Ordering;
    let (eng, id) = support::policy::peer();
    let (remote, _) = support::policy::peer();
    support::policy::connect(&eng, &id, &remote);
    support::policy::wait_connections(&eng, &id, 1);
    eng.post_settings(r#"{"ghostMode":true,"maxPeers":55}"#);
    let now = Instant::now() + Duration::from_secs(1000);
    probe(&eng, &id, now);
    let record = eng.server.records.read()[&id].clone();
    for pause in [true, false] {
        record.write().paused = pause;
        eng.server.payload_transfers_allowed.store(pause, Ordering::SeqCst);
        eng.server.client.inner.dial_limiter.set_limit(40.0);
        eng.server.ghost_mode_tick(now + Duration::from_secs(30));
        assert_eq!(eng.get_settings().effective_dial_rate_per_sec, 30, "paused={pause} payload={pause} retained probe");
    }
}

#[test]
fn test_ghost_mode_can_toggle_while_sampler_runs() {
    let (eng, _, _) = support::playback_fixture_mode(true);
    eng.post_settings(r#"{"maxPeers":55}"#);
    let server = eng.server.clone();
    let worker = std::thread::spawn(move || {
        let start = Instant::now() + Duration::from_secs(1000);
        for i in 0..200 { server.ghost_mode_tick(start + Duration::from_secs(i)); }
    });
    for _ in 0..20 {
        assert!(eng.post_settings(r#"{"ghostMode":true}"#).ghost_mode);
        assert!(!eng.post_settings(r#"{"ghostMode":false}"#).ghost_mode);
    }
    worker.join().unwrap();
    assert_eq!(eng.get_settings().effective_dial_rate_per_sec, 30, "sampler overwrote disabled base");
}

//! Ghost Mode dial-rate feedback (`engine-go/ghost_mode.go`).
//!
//! The engine aggregates swarm observations each second and calls
//! [`GhostDialController::observe`], then applies the returned dial rate to
//! its limiter. Never changes established connections or piece priorities.

use std::time::{Duration, Instant};

use crate::constants::{DEFAULT_DIAL_RATE, MAX_DIAL_RATE};

/// Swarm observation for one Ghost Mode / Ghost Pro dial tick.
#[derive(Clone, Debug, Default)]
pub struct GhostObservation {
    pub cohort: String,
    pub useful_bytes: i64,
    /// Waiting candidates and a free established-connection slot.
    pub opportunity: bool,
    pub download_limit: i64,
}

/// Closed-loop dial rate probe controller.
#[derive(Clone, Debug, Default)]
pub struct GhostDialController {
    cohort: String,
    last: Option<Instant>,
    bytes: i64,
    current: i32,
    kept: i32,
    previous: f64,
    reference: f64,
    trial_total: f64,
    trial_samples: i32,
    cooldown: Option<Instant>,
}

impl GhostDialController {
    pub fn current(&self) -> i32 {
        self.current
    }

    pub fn kept(&self) -> i32 {
        self.kept
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Returns the dial rate (peers/s) to apply now.
    pub fn observe(&mut self, now: Instant, observation: GhostObservation, base: i32) -> i32 {
        let base = base.clamp(10, MAX_DIAL_RATE);
        let reset = |c: &mut Self| -> i32 {
            *c = Self {
                cohort: observation.cohort.clone(),
                last: Some(now),
                bytes: observation.useful_bytes,
                current: base,
                kept: base,
                ..Default::default()
            };
            base
        };

        if !observation.opportunity
            || self.last.is_none()
            || self.cohort != observation.cohort
            || observation.useful_bytes < self.bytes
            || self.last.map(|t| now < t).unwrap_or(true)
            || self
                .last
                .map(|t| now.duration_since(t) > Duration::from_secs(30))
                .unwrap_or(true)
        {
            return reset(self);
        }

        let last = self.last.unwrap();
        let elapsed = now.duration_since(last);
        if elapsed < Duration::from_secs(10) {
            return self.current;
        }
        let speed = (observation.useful_bytes - self.bytes) as f64 / elapsed.as_secs_f64();
        if observation.download_limit > 0 && speed >= 0.9 * observation.download_limit as f64 {
            return reset(self);
        }
        self.last = Some(now);
        self.bytes = observation.useful_bytes;

        let back_off = |c: &mut Self| -> i32 {
            c.current = c.kept;
            c.reference = 0.0;
            c.previous = 0.0;
            c.trial_total = 0.0;
            c.trial_samples = 0;
            c.cooldown = Some(now + Duration::from_secs(60));
            c.current
        };

        if self.reference > 0.0 {
            self.trial_total += speed;
            self.trial_samples += 1;
            if speed < 0.8 * self.reference {
                return back_off(self);
            }
            if self.trial_samples < 2 {
                return self.current;
            }
            if self.trial_total / f64::from(self.trial_samples) < 1.05 * self.reference {
                return back_off(self);
            }
            self.kept = self.current;
            self.reference = 0.0;
            self.previous = 0.0;
            self.trial_total = 0.0;
            self.trial_samples = 0;
            return self.current;
        }

        if speed <= 0.0 {
            self.current = base;
            self.kept = base;
            return back_off(self);
        }
        let previous = self.previous;
        self.previous = speed;
        if self.cooldown.map(|t| now < t).unwrap_or(false)
            || self.current >= MAX_DIAL_RATE
            || previous <= 0.0
            || (speed - previous).abs() > 0.2 * previous
        {
            return self.current;
        }
        self.reference = (speed + previous) / 2.0;
        self.current = (self.kept + 10).min(MAX_DIAL_RATE);
        self.current
    }
}

/// Clamp a configured dial base the same way Ghost Mode does.
pub fn clamp_dial_base(base: i32) -> i32 {
    if base == 0 {
        DEFAULT_DIAL_RATE
    } else {
        base.clamp(10, MAX_DIAL_RATE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Trace {
        policy: GhostDialController,
        now: Instant,
        input: GhostObservation,
        base: i32,
    }

    impl Trace {
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
        let mut g = Trace::new(30);
        assert_eq!(g.step(100), 30);
        assert_eq!(g.step(100), 40);
        assert_eq!(g.step(120), 40);
        assert_eq!(g.step(120), 40);
        assert_eq!(g.policy.kept(), 40);
        assert_eq!(g.step(120), 40);
        assert_eq!(g.step(120), 50);

        let mut near = Trace::new(75);
        assert_eq!(near.step(100), 75);
        assert_eq!(near.step(100), 80);

        let mut fast = Trace::new(80);
        for _ in 0..20 {
            assert_eq!(fast.step(100), 80);
        }
    }

    #[test]
    fn test_ghost_mode_rolls_back_failed_probes_and_cools_down() {
        for speed in [100i64, 60] {
            let mut g = Trace::new(30);
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
        let mut g = Trace::new(30);
        for speed in [0i64, 100, 1000, 10, 1000, 0, 0] {
            assert_eq!(g.step(speed), 30);
        }
    }

    #[test]
    fn test_ghost_mode_resets_for_changed_conditions() {
        for condition in [
            "no spare slots",
            "cohort changed",
            "counter reset",
            "long gap",
            "download capped",
        ] {
            let mut g = Trace::new(30);
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
}

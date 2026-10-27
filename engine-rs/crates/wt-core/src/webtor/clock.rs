//! Injectable clock for deterministic seek-decision tests.

use std::time::{Duration, Instant};

/// Wall or virtual time. Decision functions take `&dyn Clock` (or a generic)
/// so tests can advance time without sleeping.
pub trait Clock {
    fn now(&self) -> Instant;
}

/// Real wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Manual clock for tests: starts at a frozen base and advances on demand.
#[derive(Clone, Debug)]
pub struct ManualClock {
    base: Instant,
    elapsed: Duration,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn advance(&mut self, d: Duration) {
        self.elapsed = self.elapsed.saturating_add(d);
    }

    pub fn set_elapsed(&mut self, d: Duration) {
        self.elapsed = d;
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base + self.elapsed
    }
}

/// Age of `then` relative to `clock.now()`, saturating at zero.
pub fn age(clock: &dyn Clock, then: Instant) -> Duration {
    clock.now().saturating_duration_since(then)
}

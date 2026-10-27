//! Round-trip estimator from requests sent to an idle peer (`rttEstimator`).

use std::time::{Duration, Instant};

use wt_types::RequestIndex;

use super::clock::{age, Clock};

/// Measures a peer's round trip from requests sent while nothing else was
/// queued, so the time is the trip and one block, not a queue.
///
/// Matches Go `rttEstimator`: estimate starts at zero until the first sample.
#[derive(Clone, Debug, Default)]
pub struct RttEstimator {
    pub probe: Option<RequestIndex>,
    pub probe_at: Option<Instant>,
    pub estimate: Duration,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `sent`. A probe whose block never came (cancelled, rejected) is
    /// replaced after 10 s.
    pub fn sent(&mut self, r: RequestIndex, clock: &dyn Clock) {
        let replace = match self.probe_at {
            None => true,
            Some(at) => age(clock, at) > Duration::from_secs(10),
        };
        if replace {
            self.probe = Some(r);
            self.probe_at = Some(clock.now());
        }
    }

    /// Go: `received`. Only the probe request updates the estimate; EWMA /8.
    pub fn received(&mut self, r: RequestIndex, clock: &dyn Clock) {
        let (Some(probe), Some(at)) = (self.probe, self.probe_at) else {
            return;
        };
        if r != probe {
            return;
        }
        let sample = age(clock, at);
        self.probe_at = None;
        self.probe = None;
        if self.estimate.is_zero() {
            self.estimate = sample;
        } else {
            // estimate += (sample - estimate) / 8
            let est = self.estimate.as_secs_f64();
            let s = sample.as_secs_f64();
            self.estimate = Duration::from_secs_f64(est + (s - est) / 8.0);
        }
    }
}

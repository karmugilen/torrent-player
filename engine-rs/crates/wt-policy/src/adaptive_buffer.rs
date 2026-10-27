//! Adaptive playback look-ahead (`engine-go/adaptive_buffer.go`).
//!
//! The engine measures reader demand (bytes/s) after each sample second and
//! calls [`AdaptiveBuffer::observe`]. The returned window size becomes the
//! reader's forward budget for that tick. Pass the same controller across
//! samples; create a fresh one per playback reader.

use crate::constants::PLAYBACK_URGENT_BYTES;

/// Bounded headroom controller for variable reader demand.
///
/// Feedback uses measured rates. With `burst_aware`, dual fast/slow EWMA
/// tracks keep headroom across longer bursts.
#[derive(Clone, Debug, Default)]
pub struct AdaptiveBuffer {
    mean: f64,
    variance: f64,
    window: i64,
    burst_aware: bool,
    fast_mean: f64,
    fast_variance: f64,
    slow_mean: f64,
    slow_variance: f64,
}

impl AdaptiveBuffer {
    pub fn new(burst_aware: bool) -> Self {
        Self {
            burst_aware,
            ..Default::default()
        }
    }

    pub fn burst_aware(&self) -> bool {
        self.burst_aware
    }

    pub fn set_burst_aware(&mut self, on: bool) {
        self.burst_aware = on;
    }

    pub fn window(&self) -> i64 {
        self.window
    }

    /// Update with the latest measured demand rate and return the damped window.
    ///
    /// `limit` is the storage/scheduler ceiling for this reader.
    pub fn observe(&mut self, bytes_per_second: f64, limit: i64) -> i64 {
        if self.mean == 0.0 {
            self.mean = bytes_per_second;
        } else {
            let delta = bytes_per_second - self.mean;
            self.mean += 0.2 * delta;
            self.variance = 0.8 * (self.variance + 0.2 * delta * delta);
        }
        // Eight seconds of demand plus a variability margin.
        let mut forecast = self.mean + 2.0 * self.variance.sqrt();
        if self.burst_aware {
            update_ewma(
                &mut self.fast_mean,
                &mut self.fast_variance,
                bytes_per_second,
                0.5,
            );
            update_ewma(
                &mut self.slow_mean,
                &mut self.slow_variance,
                bytes_per_second,
                0.05,
            );
            forecast = forecast
                .max(self.fast_mean + 2.0 * self.fast_variance.sqrt())
                .max(self.slow_mean + 2.0 * self.slow_variance.sqrt());
        }
        let mut target = ((8.0 * forecast).min(limit as f64)) as i64;
        target = target.max(PLAYBACK_URGENT_BYTES * 2); // 4 MiB floor matches Go `4<<20`
        if self.window <= 0 {
            self.window = target;
        }
        let step = (self.window / 4).max(1);
        if target > self.window + self.window / 10 {
            self.window = target.min(self.window + step);
        } else if target < self.window - self.window / 10 {
            self.window = target.max(self.window - step);
        }
        self.window = self.window.max(4 << 20).min(limit);
        self.window
    }
}

fn update_ewma(mean: &mut f64, variance: &mut f64, sample: f64, weight: f64) {
    if *mean == 0.0 {
        *mean = sample;
        return;
    }
    let delta = sample - *mean;
    *mean += weight * delta;
    *variance = (1.0 - weight) * (*variance + weight * delta * delta);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_pattern(burst: bool, pattern: &[f64], settle: bool) -> i64 {
        let mut c = AdaptiveBuffer::new(burst);
        let limit = 64 << 20;
        let mut count = 20;
        if settle {
            count += 20;
        }
        let mut window = 0;
        for sample in 0..count {
            let mut rate = pattern[sample % pattern.len()];
            if settle && sample >= 20 {
                rate = (1 << 20) as f64;
            }
            window = c.observe(rate, limit);
        }
        window
    }

    #[test]
    fn adaptive_adds_headroom_for_variable_demand() {
        let variable = [256.0 * 1024.0, 1792.0 * 1024.0];
        // Plain: fixed 26 MiB-ish floor without adaptation — use a controller
        // that never saw variance by feeding only the mean of the pattern.
        let mut plain = AdaptiveBuffer::new(false);
        let mean = (variable[0] + variable[1]) / 2.0;
        let mut plain_w = 0;
        for _ in 0..20 {
            plain_w = plain.observe(mean, 64 << 20);
        }
        let adaptive = run_pattern(false, &variable, false);
        assert!(
            adaptive > plain_w,
            "variable demand needs extra look-ahead: adaptive={adaptive} plain={plain_w}"
        );
        let settled = run_pattern(false, &variable, true);
        assert!(
            settled < adaptive,
            "stable demand should release extra headroom: stable={settled} variable={adaptive}"
        );
    }

    #[test]
    fn burst_aware_retains_headroom_across_longer_bursts() {
        let pattern = [256.0 * 1024.0, 1792.0 * 1024.0];
        let adaptive = run_pattern(false, &pattern, true);
        let burst = run_pattern(true, &pattern, true);
        assert!(
            burst > adaptive,
            "longer burst history needs headroom: burst={burst} adaptive={adaptive}"
        );
    }
}

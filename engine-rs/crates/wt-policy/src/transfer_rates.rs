//! Per-torrent transfer rate sampling (`engine-go/transfer_rates.go`).
//!
//! Each torrent owns its samples. Call [`TransferRates::sample`] from the
//! engine's status tick with injected `now` and cumulative byte counters.

use std::time::Instant;

/// EWMA download/upload rates for one torrent.
#[derive(Clone, Debug, Default)]
pub struct TransferRates {
    last_sample: Option<Instant>,
    downloaded: i64,
    uploaded: i64,
    pub down: i64,
    pub up: i64,
}

impl TransferRates {
    pub fn down(&self) -> i64 {
        self.down
    }

    pub fn up(&self) -> i64 {
        self.up
    }

    pub fn sample(&mut self, now: Instant, downloaded: i64, uploaded: i64, paused: bool) {
        let elapsed = self
            .last_sample
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        if self.last_sample.is_none() || downloaded < self.downloaded || uploaded < self.uploaded {
            self.down = 0;
            self.up = 0;
        } else if elapsed >= 0.5 {
            self.down =
                smooth_transfer_rate(self.down, (downloaded - self.downloaded) as f64 / elapsed);
            self.up = smooth_transfer_rate(self.up, (uploaded - self.uploaded) as f64 / elapsed);
        } else {
            return;
        }
        if paused {
            self.down = 0;
            self.up = 0;
        }
        self.last_sample = Some(now);
        self.downloaded = downloaded;
        self.uploaded = uploaded;
    }
}

fn smooth_transfer_rate(previous: i64, current: f64) -> i64 {
    if previous == 0 {
        current as i64
    } else {
        (0.6 * previous as f64 + 0.4 * current) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_transfer_rates_handle_irregular_samples_pause_and_counter_reset() {
        let start = Instant::now();
        let mut rates = TransferRates::default();
        rates.sample(start, 1000, 100, false);
        rates.sample(start + Duration::from_secs(2), 5000, 300, false);
        assert_eq!(rates.down, 2000);
        assert_eq!(rates.up, 100);

        rates.sample(start + Duration::from_millis(2100), 5100, 310, false);
        assert_eq!(rates.downloaded, 5000);
        assert_eq!(rates.down, 2000);

        rates.sample(start + Duration::from_secs(3), 5100, 310, true);
        assert_eq!(rates.down, 0);
        assert_eq!(rates.up, 0);

        rates.sample(start + Duration::from_secs(4), 6100, 410, false);
        assert_eq!(rates.down, 1000);
        assert_eq!(rates.up, 100);

        rates.sample(start + Duration::from_secs(5), 0, 0, false);
        assert_eq!(rates.down, 0);
        assert_eq!(rates.up, 0);
    }

    #[test]
    fn test_torrent_rates_and_eta_use_their_own_samples() {
        let start = Instant::now();
        let mut a = TransferRates {
            last_sample: Some(start),
            ..Default::default()
        };
        let mut b = TransferRates {
            last_sample: Some(start),
            ..Default::default()
        };
        a.sample(start + Duration::from_secs(1), 0, 0, false);
        b.sample(start + Duration::from_secs(1), 1000, 100, false);
        assert_eq!(a.down, 0);
        assert_eq!(b.down, 1000);
        assert_eq!(b.up, 100);
    }
}

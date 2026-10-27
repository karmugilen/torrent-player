//! # Rate limiting (agy-2 integration guide)
//!
//! Token-bucket limiter matching `golang.org/x/time/rate` as used by
//! anacrolix (`UploadRateLimiter` / `DownloadRateLimiter` / `DialRateLimiter`).
//!
//! ## What agy-2 should call
//!
//! ```ignore
//! use wt_core::ratelimit::{Limiter, INF, DEFAULT_DOWNLOAD_BURST};
//!
//! // Unlimited download/upload (Go: rate.NewLimiter(rate.Inf, 0) then burst→1MiB):
//! let download = Limiter::infinite();
//! download.ensure_default_burst(DEFAULT_DOWNLOAD_BURST); // 1 MiB
//!
//! // Dial: rate == burst (Go: rate.NewLimiter(10, 10) or dial_rate):
//! let dial = Limiter::new(100.0, 100);
//!
//! // Hot path (piece bytes / dials):
//! if download.allow_n(chunk_len) { /* read */ }
//! download.wait_n(chunk_len).await; // async backpressure
//!
//! // Runtime reconfigure (engine settings):
//! download.set_limit(512.0 * 1024.0); // bytes/s
//! download.set_burst(1 << 20);
//! ```

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Infinite rate (`rate.Inf`).
pub const INF: f64 = f64::INFINITY;

/// Default download/upload burst when burst was left at 0 (Go: 1 MiB).
pub const DEFAULT_DOWNLOAD_BURST: usize = 1 << 20;

#[derive(Debug)]
struct Inner {
    /// Tokens per second. `INF` = unlimited.
    limit: f64,
    burst: usize,
    tokens: f64,
    last: Instant,
}

/// Shared token-bucket limiter (`*rate.Limiter`).
#[derive(Clone, Debug)]
pub struct Limiter {
    inner: Arc<Mutex<Inner>>,
}

impl Limiter {
    /// `rate.NewLimiter(limit, burst)`.
    pub fn new(limit: f64, burst: usize) -> Self {
        let tokens = burst as f64;
        Self {
            inner: Arc::new(Mutex::new(Inner {
                limit,
                burst,
                tokens,
                last: Instant::now(),
            })),
        }
    }

    /// `rate.NewLimiter(rate.Inf, 0)`.
    pub fn infinite() -> Self {
        Self::new(INF, 0)
    }

    /// If burst is 0, set it to `def` (Go `setRateLimiterBurstIfZero`).
    pub fn ensure_default_burst(&self, def: usize) {
        let mut g = self.inner.lock();
        if g.burst == 0 {
            g.burst = def;
            if g.tokens < def as f64 {
                g.tokens = def as f64;
            }
        }
    }

    pub fn limit(&self) -> f64 {
        self.inner.lock().limit
    }

    pub fn burst(&self) -> usize {
        self.inner.lock().burst
    }

    pub fn tokens(&self) -> f64 {
        let mut g = self.inner.lock();
        g.advance(Instant::now());
        g.tokens
    }

    pub fn set_limit(&self, limit: f64) {
        let mut g = self.inner.lock();
        g.advance(Instant::now());
        g.limit = limit;
    }

    pub fn set_burst(&self, burst: usize) {
        let mut g = self.inner.lock();
        g.advance(Instant::now());
        g.burst = burst;
        if g.tokens > burst as f64 {
            g.tokens = burst as f64;
        }
    }

    /// Non-blocking `Allow()` (one token).
    pub fn allow(&self) -> bool {
        self.allow_n(1)
    }

    /// Non-blocking `AllowN(now, n)`.
    pub fn allow_n(&self, n: usize) -> bool {
        if n == 0 {
            return true;
        }
        let mut g = self.inner.lock();
        let now = Instant::now();
        g.advance(now);
        if g.limit.is_infinite() {
            return true;
        }
        if g.tokens >= n as f64 {
            g.tokens -= n as f64;
            true
        } else {
            false
        }
    }

    /// Delay until `n` tokens are available (`ReserveN(...).Delay()`), or `None`
    /// if the request can never be satisfied (`n > burst` with finite limit).
    pub fn reserve_delay(&self, n: usize) -> Option<Duration> {
        if n == 0 {
            return Some(Duration::ZERO);
        }
        let mut g = self.inner.lock();
        let now = Instant::now();
        g.advance(now);
        if g.limit.is_infinite() {
            return Some(Duration::ZERO);
        }
        if n > g.burst {
            return None;
        }
        if g.tokens >= n as f64 {
            g.tokens -= n as f64;
            return Some(Duration::ZERO);
        }
        let need = n as f64 - g.tokens;
        let secs = need / g.limit;
        g.tokens = 0.0;
        // Advance last so subsequent calls account for the reservation time.
        g.last = now + duration_from_secs_f64(secs);
        Some(duration_from_secs_f64(secs))
    }

    /// Async wait until `n` tokens are available (`WaitN`).
    pub async fn wait_n(&self, n: usize) {
        loop {
            match self.reserve_delay(n) {
                Some(d) if d.is_zero() => return,
                Some(d) => sleep(d).await,
                None => {
                    // Can never satisfy: wait briefly and retry in case burst grows.
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}

impl Inner {
    fn advance(&mut self, now: Instant) {
        if self.limit.is_infinite() {
            self.tokens = self.burst as f64;
            self.last = now;
            return;
        }
        if self.limit <= 0.0 {
            self.last = now;
            return;
        }
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.limit).min(self.burst as f64);
            self.last = now;
        }
    }
}

fn duration_from_secs_f64(secs: f64) -> Duration {
    if secs <= 0.0 {
        Duration::ZERO
    } else if secs.is_infinite() {
        Duration::from_secs(u64::MAX / 4)
    } else {
        Duration::from_secs_f64(secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infinite_always_allows() {
        let l = Limiter::infinite();
        assert!(l.allow_n(1_000_000));
        l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
        assert_eq!(l.burst(), DEFAULT_DOWNLOAD_BURST);
        assert!(l.allow_n(DEFAULT_DOWNLOAD_BURST));
    }

    #[test]
    fn finite_bucket_and_refill() {
        let l = Limiter::new(100.0, 10);
        assert!(l.allow_n(10));
        assert!(!l.allow());
        // After ~50ms should have ~5 tokens at 100/s
        std::thread::sleep(Duration::from_millis(60));
        assert!(l.allow_n(5));
        assert!(!l.allow_n(6));
    }

    #[test]
    fn set_limit_and_burst_runtime() {
        let l = Limiter::new(INF, 0);
        l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
        l.set_limit(1024.0);
        l.set_burst(2048);
        assert_eq!(l.limit(), 1024.0);
        assert_eq!(l.burst(), 2048);
    }

    #[test]
    fn reserve_delay_none_when_over_burst() {
        let l = Limiter::new(10.0, 5);
        assert!(l.reserve_delay(6).is_none());
    }

    #[tokio::test]
    async fn wait_n_eventually() {
        let l = Limiter::new(1000.0, 1);
        assert!(l.allow());
        l.wait_n(1).await;
    }
}

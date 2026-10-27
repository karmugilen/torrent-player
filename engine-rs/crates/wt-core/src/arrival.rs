//! Learned block-arrival latency estimation from `engine-go/patches/torrent/webtor_arrival.go`.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const ARRIVAL_FEATURES: usize = 8;
pub const ARRIVAL_HIDDEN: usize = 8;
pub const ARRIVAL_NET_STEP: f64 = 0.005;
pub const ARRIVAL_SCORE_ALPHA: f64 = 0.01;
pub const ARRIVAL_PEER_ALPHA: f64 = 0.1;
pub const ARRIVAL_IDLE_CAP: f64 = 10.0;
pub const ARRIVAL_UNCHOKE_CAP: f64 = 30.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArrivalNet {
    pub w1: [[f64; ARRIVAL_FEATURES]; ARRIVAL_HIDDEN],
    pub w2: [f64; ARRIVAL_HIDDEN],
    pub b2: f64,
}

impl Default for ArrivalNet {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrivalNet {
    pub fn new() -> Self {
        let mut net = Self {
            w1: [[0.0; ARRIVAL_FEATURES]; ARRIVAL_HIDDEN],
            w2: [0.0; ARRIVAL_HIDDEN],
            b2: (0.5_f64).ln(),
        };
        // Reproduce Go's rand.New(rand.NewSource(1)).NormFloat64()
        // Simple deterministic LCG + Box-Muller to give stable initialization
        let mut seed: u64 = 1;
        let mut next_norm = move || -> f64 {
            // Marsaglia polar method or Box-Muller using LCG
            loop {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let u1 = ((seed >> 11) as f64) / ((1u64 << 53) as f64);
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let u2 = ((seed >> 11) as f64) / ((1u64 << 53) as f64);
                if u1 > 1e-15 {
                    return (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
                }
            }
        };

        for h in 0..ARRIVAL_HIDDEN {
            for i in 0..ARRIVAL_FEATURES {
                net.w1[h][i] = 0.3 * next_norm();
            }
        }
        net
    }

    pub fn net_inputs(x: [f64; ARRIVAL_FEATURES]) -> [f64; ARRIVAL_FEATURES] {
        let mut z = [0.0; ARRIVAL_FEATURES];
        z[0] = 1.0;
        for i in 1..ARRIVAL_FEATURES {
            z[i] = (1.0 + 10.0 * x[i].max(0.0)).ln();
        }
        z
    }

    #[allow(clippy::needless_range_loop)]
    pub fn forward(&self, z: [f64; ARRIVAL_FEATURES]) -> (f64, [f64; ARRIVAL_HIDDEN]) {
        let mut y = self.b2;
        let mut h = [0.0; ARRIVAL_HIDDEN];
        for j in 0..ARRIVAL_HIDDEN {
            let mut a = 0.0;
            for i in 0..ARRIVAL_FEATURES {
                a += self.w1[j][i] * z[i];
            }
            h[j] = a.tanh();
            y += self.w2[j] * h[j];
        }
        (y, h)
    }

    pub fn predict(&self, x: [f64; ARRIVAL_FEATURES]) -> f64 {
        let (y, _) = self.forward(Self::net_inputs(x));
        let clamped = y.clamp((0.001_f64).ln(), (60.0_f64).ln());
        clamped.exp()
    }

    #[allow(clippy::needless_range_loop)]
    pub fn learn(&mut self, x: [f64; ARRIVAL_FEATURES], actual: f64) {
        let z = Self::net_inputs(x);
        let (y, h) = self.forward(z);
        let target = actual.max(0.001).ln();
        let err = (target - y).clamp(-2.0, 2.0);
        for j in 0..ARRIVAL_HIDDEN {
            let back = err * self.w2[j] * (1.0 - h[j] * h[j]);
            self.w2[j] += ARRIVAL_NET_STEP * err * h[j];
            for i in 0..ARRIVAL_FEATURES {
                self.w1[j][i] += ARRIVAL_NET_STEP * back * z[i];
            }
        }
        self.b2 += ARRIVAL_NET_STEP * err;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrivalStats {
    pub samples: u64,
    pub mean_abs_error_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArrivalModel {
    pub net: ArrivalNet,
    pub samples: u64,
    pub net_err: f64,
}

impl Default for ArrivalModel {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrivalModel {
    pub fn new() -> Self {
        Self {
            net: ArrivalNet::new(),
            samples: 0,
            net_err: 0.0,
        }
    }

    pub fn on_block_learned(&mut self, x: [f64; ARRIVAL_FEATURES], estimate: f64, actual: f64) {
        let d = (estimate - actual).abs();
        if self.samples == 0 {
            self.net_err = d;
        } else {
            self.net_err += ARRIVAL_SCORE_ALPHA * (d - self.net_err);
        }
        self.samples += 1;
        self.net.learn(x, actual);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ArrivalSample {
    pub sent: Instant,
    pub x: [f64; ARRIVAL_FEATURES],
    pub estimate: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ArrivalPeer {
    pub jitter: f64,
    pub recent: f64,
    pub last_block: Option<Instant>,
}

impl ArrivalPeer {
    pub fn on_block_arrived(&mut self, err: f64) {
        self.jitter += ARRIVAL_PEER_ALPHA * (err - self.jitter);
        self.last_block = Some(Instant::now());
    }

    pub fn overdue_duration(&self, estimate: f64, min: Duration) -> Duration {
        let late_sec = estimate + 2.0 * self.jitter;
        let late_dur = Duration::from_secs_f64(late_sec.max(0.0));
        late_dur.max(min)
    }
}

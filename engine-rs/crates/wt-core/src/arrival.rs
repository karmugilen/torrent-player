//! Learned block-arrival latency estimation from `engine-go/patches/torrent/webtor_arrival.go`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    #[serde(rename = "W1", alias = "w1")]
    pub w1: [[f64; ARRIVAL_FEATURES]; ARRIVAL_HIDDEN],
    #[serde(rename = "W2", alias = "w2")]
    pub w2: [f64; ARRIVAL_HIDDEN],
    #[serde(rename = "B2", alias = "b2")]
    pub b2: f64,
}

impl Default for ArrivalNet {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrivalNet {
    pub fn new() -> Self {
        // Go rand.New(rand.NewSource(1)).NormFloat64(), scaled by 0.3.
        // Identical initial weights keep cold-model comparisons meaningful.
        Self {
            w1: [
                [
                    -0.37012745327938407,
                    -0.037904253210711877,
                    -0.15629837134594507,
                    0.68571573530987406,
                    0.096841575783473965,
                    0.17702018627990812,
                    0.047642322052930682,
                    0.29676062528867453,
                ],
                [
                    -0.21938490485324372,
                    0.2059142355107918,
                    0.4756211886841869,
                    0.25146177132624314,
                    0.38965225425523026,
                    0.15820751791795848,
                    0.21973257774135393,
                    -0.32195394632662572,
                ],
                [
                    0.21003627073199543,
                    0.12945921560881596,
                    0.29988783630337873,
                    -0.45719030175836795,
                    -0.094961172868226473,
                    0.56683926187904443,
                    0.33021875812500623,
                    -0.29782295722543101,
                ],
                [
                    0.29691312606255948,
                    -0.18456704558332948,
                    -0.43051407663966845,
                    -0.64543100482279336,
                    0.04120511207200523,
                    0.13284678810796999,
                    -0.2538283120366791,
                    -0.024838510240844758,
                ],
                [
                    0.046837948584844392,
                    -0.43518697565114134,
                    0.08392311559895145,
                    -0.52167191934254775,
                    0.21082507846865436,
                    0.10384166884646871,
                    -0.32086362070590613,
                    -0.24997557275692203,
                ],
                [
                    0.099086815683798526,
                    0.52372251953338422,
                    -0.33621422770698994,
                    0.22703965255400888,
                    0.27818954122922496,
                    -0.43665174340570062,
                    0.29191031455295557,
                    -0.088564096147632679,
                ],
                [
                    0.15303483314580482,
                    -0.14164104254876037,
                    0.075462013217744761,
                    -0.024581241460070735,
                    0.050007215675489269,
                    0.10921353555916666,
                    -0.49159232335693032,
                    0.25070050710777408,
                ],
                [
                    0.34626924411301113,
                    -0.022510904284437964,
                    -0.23116828435493814,
                    -0.3369879330777466,
                    -0.23286396296213011,
                    0.20158313090299437,
                    0.42608384903666402,
                    -0.11063353822420875,
                ],
            ],
            w2: [0.0; ARRIVAL_HIDDEN],
            b2: (0.5_f64).ln(),
        }
    }

    pub fn net_inputs(x: [f64; ARRIVAL_FEATURES]) -> [f64; ARRIVAL_FEATURES] {
        let mut z = [0.0; ARRIVAL_FEATURES];
        z[0] = 1.0;
        for i in 1..ARRIVAL_FEATURES {
            z[i] = (10.0 * x[i].max(0.0)).ln_1p();
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
    #[serde(rename = "Net", alias = "net")]
    pub net: ArrivalNet,
    #[serde(rename = "Samples", alias = "samples")]
    pub samples: u64,
    #[serde(rename = "NetErr", alias = "net_err")]
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
    /// Go webtorNoteBlock: recent rate updates even without a request sample.
    pub fn note_block(&mut self, chunk_size: usize) {
        let now = Instant::now();
        if let Some(last) = self.last_block {
            let rate = chunk_size as f64 / now.duration_since(last).as_secs_f64().max(0.001);
            if self.recent == 0.0 {
                self.recent = rate;
            } else {
                self.recent += ARRIVAL_PEER_ALPHA * (rate - self.recent);
            }
        }
        self.last_block = Some(now);
    }

    pub fn inputs(
        &self,
        rate: f64,
        rtt: Duration,
        queued: usize,
        chunk_size: usize,
        unchoked_at: Option<Instant>,
    ) -> Option<[f64; ARRIVAL_FEATURES]> {
        if rate <= 0.0 || chunk_size == 0 {
            return None;
        }
        let rtt = if rtt.is_zero() {
            0.2
        } else {
            rtt.as_secs_f64()
        };
        let block = chunk_size as f64 / rate;
        let recent = if self.recent > 0.0 { self.recent } else { rate };
        Some([
            1.0,
            rtt,
            queued as f64 * block,
            block,
            self.jitter,
            queued as f64 * chunk_size as f64 / recent,
            self.last_block
                .map(|t| t.elapsed().as_secs_f64().min(ARRIVAL_IDLE_CAP))
                .unwrap_or(0.0),
            unchoked_at
                .map(|t| t.elapsed().as_secs_f64().min(ARRIVAL_UNCHOKE_CAP))
                .unwrap_or(0.0),
        ])
    }

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

/// Shared with the scheduler so another peer can inspect a holder's learned
/// deadline without locking its connection handler or the global model.
#[derive(Default)]
pub struct PeerArrivalState {
    pub peer: ArrivalPeer,
    pub unchoked_at: Option<Instant>,
    pub samples: HashMap<wt_types::RequestIndex, ArrivalSample>,
}

impl PeerArrivalState {
    pub fn overdue(&self, req: wt_types::RequestIndex, min: Duration) -> Option<bool> {
        let sample = self.samples.get(&req)?;
        Some(sample.sent.elapsed() >= self.peer.overdue_duration(sample.estimate, min))
    }
}

//! Connection and torrent statistics.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};
use wt_types::{PeerId, PeerSource, Transport};

/// Byte/chunk counters (`torrent.ConnStats`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnStats {
    pub bytes_written: i64,
    pub bytes_written_data: i64,
    pub bytes_read: i64,
    pub bytes_read_data: i64,
    pub bytes_read_useful_data: i64,
    pub bytes_read_useful_intended_data: i64,
    pub chunks_written: i64,
    pub chunks_read: i64,
    pub chunks_read_useful: i64,
    pub chunks_read_wasted: i64,
    pub pieces_dirtied_good: i64,
    pub pieces_dirtied_bad: i64,
}

impl ConnStats {
    pub fn add(&mut self, other: &ConnStats) {
        self.bytes_written += other.bytes_written;
        self.bytes_written_data += other.bytes_written_data;
        self.bytes_read += other.bytes_read;
        self.bytes_read_data += other.bytes_read_data;
        self.bytes_read_useful_data += other.bytes_read_useful_data;
        self.bytes_read_useful_intended_data += other.bytes_read_useful_intended_data;
        self.chunks_written += other.chunks_written;
        self.chunks_read += other.chunks_read;
        self.chunks_read_useful += other.chunks_read_useful;
        self.chunks_read_wasted += other.chunks_read_wasted;
        self.pieces_dirtied_good += other.pieces_dirtied_good;
        self.pieces_dirtied_bad += other.pieces_dirtied_bad;
    }
}

/// Atomic counters backing a live connection's `ConnStats`.
#[derive(Default)]
pub struct AtomicConnStats {
    pub bytes_written: AtomicI64,
    pub bytes_written_data: AtomicI64,
    pub bytes_read: AtomicI64,
    pub bytes_read_data: AtomicI64,
    pub bytes_read_useful_data: AtomicI64,
    pub bytes_read_useful_intended_data: AtomicI64,
    pub chunks_written: AtomicI64,
    pub chunks_read: AtomicI64,
    pub chunks_read_useful: AtomicI64,
    pub chunks_read_wasted: AtomicI64,
    pub pieces_dirtied_good: AtomicI64,
    pub pieces_dirtied_bad: AtomicI64,
}

impl AtomicConnStats {
    pub fn snapshot(&self) -> ConnStats {
        ConnStats {
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            bytes_written_data: self.bytes_written_data.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_read_data: self.bytes_read_data.load(Ordering::Relaxed),
            bytes_read_useful_data: self.bytes_read_useful_data.load(Ordering::Relaxed),
            bytes_read_useful_intended_data: self
                .bytes_read_useful_intended_data
                .load(Ordering::Relaxed),
            chunks_written: self.chunks_written.load(Ordering::Relaxed),
            chunks_read: self.chunks_read.load(Ordering::Relaxed),
            chunks_read_useful: self.chunks_read_useful.load(Ordering::Relaxed),
            chunks_read_wasted: self.chunks_read_wasted.load(Ordering::Relaxed),
            pieces_dirtied_good: self.pieces_dirtied_good.load(Ordering::Relaxed),
            pieces_dirtied_bad: self.pieces_dirtied_bad.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClientStats {
    pub conn_stats: ConnStats,
    pub active_half_open: usize,
}

#[derive(Clone, Debug, Default)]
pub struct TorrentStats {
    pub conn_stats: ConnStats,
    pub total_peers: usize,
    pub pending_peers: usize,
    pub active_peers: usize,
    pub connected_seeders: usize,
    pub half_open_peers: usize,
    pub pieces_complete: usize,
}

/// Snapshot of one live connection.
#[derive(Clone, Debug)]
pub struct PeerConnInfo {
    pub remote_addr: SocketAddr,
    pub dial_addr: Option<SocketAddr>,
    pub peer_id: Option<PeerId>,
    pub client_name: String,
    pub transport: Transport,
    pub source: PeerSource,
    pub encrypted: bool,
    pub outgoing: bool,
    pub peer_choking: bool,
    pub am_choking: bool,
    pub peer_interested: bool,
    pub am_interested: bool,
    pub conn_stats: ConnStats,
    pub download_rate: f64,
    pub upload_rate: f64,
    pub peer_completion: f64,
    pub pending_requests: usize,
    pub peer_max_requests: usize,
    pub peak_requests: usize,
    pub pipeline_limit: usize,
    pub nominal_max_requests: usize,
    pub rtt: Duration,
    pub queued: usize,
    pub cancelled: usize,
    pub connected_for: Duration,
    pub last_useful_data: Option<Duration>,
}

/// Tracks rolling throughput with EMA or windowed rates.
#[derive(Clone, Debug)]
pub struct RateEstimator {
    last_update: Instant,
    bytes_accum: u64,
    rate_bps: f64,
}

impl Default for RateEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl RateEstimator {
    pub fn new() -> Self {
        Self {
            last_update: Instant::now(),
            bytes_accum: 0,
            rate_bps: 0.0,
        }
    }

    pub fn set_rate(&mut self, rate_bps: f64) {
        self.rate_bps = rate_bps;
        self.last_update = Instant::now();
    }

    pub fn add_bytes(&mut self, n: usize) {
        self.bytes_accum += n as u64;
        let elapsed = self.last_update.elapsed();
        if elapsed >= Duration::from_millis(100) {
            let inst_rate = (self.bytes_accum as f64) / elapsed.as_secs_f64();
            if self.rate_bps == 0.0 {
                self.rate_bps = inst_rate;
            } else {
                // EMA alpha ~ 0.25
                self.rate_bps = 0.25 * inst_rate + 0.75 * self.rate_bps;
            }
            self.bytes_accum = 0;
            self.last_update = Instant::now();
        }
    }

    pub fn rate(&self) -> f64 {
        if self.rate_bps > 0.0 {
            let elapsed = self.last_update.elapsed();
            if elapsed >= Duration::from_secs(3) {
                return 0.0;
            }
            return self.rate_bps;
        }
        if self.bytes_accum > 0 {
            let elapsed = self.last_update.elapsed().as_secs_f64();
            if elapsed > 0.0 {
                return self.bytes_accum as f64 / elapsed;
            }
        }
        0.0
    }
}

/// Go `Peer.downloadRate`: useful bytes over the time spent expecting chunks
/// (requests outstanding, interested, not choked), summed over the whole
/// connection. Idle gaps do not count, so a peer that briefly runs out of
/// requests keeps its rate instead of collapsing to zero.
#[derive(Clone, Debug, Default)]
pub struct ExpectingRate {
    useful_bytes: u64,
    expecting_total: Duration,
    expecting_since: Option<Instant>,
}

impl ExpectingRate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Go `updateExpectingChunks`.
    pub fn set_expecting(&mut self, expecting: bool) {
        match (expecting, self.expecting_since) {
            (true, None) => self.expecting_since = Some(Instant::now()),
            (false, Some(since)) => {
                self.expecting_total += since.elapsed();
                self.expecting_since = None;
            }
            _ => {}
        }
    }

    /// Go `BytesReadUsefulData`.
    pub fn add_bytes(&mut self, n: usize) {
        self.useful_bytes += n as u64;
    }

    /// Go `totalExpectingTime`.
    pub fn expecting_time(&self) -> Duration {
        self.expecting_total + self.expecting_since.map_or(Duration::ZERO, |s| s.elapsed())
    }

    pub fn rate(&self) -> f64 {
        if self.useful_bytes == 0 {
            return 0.0;
        }
        let secs = self.expecting_time().as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.useful_bytes as f64 / secs
    }
}

/// Token bucket rate limiter for dialing and bandwidth limiting.
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: parking_lot::Mutex<(f64, Instant)>,
}

impl TokenBucket {
    pub fn new(rate: f64, burst: f64) -> Self {
        Self {
            rate,
            burst,
            tokens: parking_lot::Mutex::new((burst, Instant::now())),
        }
    }

    pub fn try_acquire(&self, count: f64) -> bool {
        let mut lock = self.tokens.lock();
        let (tokens, last_time) = &mut *lock;
        let now = Instant::now();
        let elapsed = now.duration_since(*last_time).as_secs_f64();
        *last_time = now;
        *tokens = (*tokens + elapsed * self.rate).min(self.burst);

        if *tokens >= count {
            *tokens -= count;
            true
        } else {
            false
        }
    }

    pub async fn acquire(&self, count: f64) {
        while !self.try_acquire(count) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Cumulative swarm diagnostics. Counters are per torrent; gauges are sampled.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmDiagnostics {
    pub pool_size: usize,
    pub dialing: usize,
    pub established: usize,
    pub dial_ok: u64,
    pub dial_failures: std::collections::BTreeMap<String, u64>,
    pub closes: std::collections::BTreeMap<String, u64>,
    pub pex_received: u64,
    pub pex_dialed: u64,
}

pub(crate) fn failure_kind(reason: &str) -> &'static str {
    let reason = reason.to_ascii_lowercase();
    // Go dialAndCompleteHandshake reports race failure as "all initial dials failed"
    // (dialFailures=other); per-dialer timeouts go to transportFailures.
    if reason.contains("all initial dials failed") { "other" }
    else if reason.contains("refused") { "refused" }
    else if reason.contains("timeout") || reason.contains("timed out") || reason.contains("deadline") { "timeout" }
    else if reason.contains("mse") || reason.contains("obfuscation") { "mse" }
    else if reason.contains("handshake") { "handshake" }
    else if reason.contains("eof") { "eof" }
    else if reason.contains("decode") { "decode" }
    else if reason.contains("choker") { "choker" }
    else if reason.contains("reset") { "reset" }
    else { "other" }
}

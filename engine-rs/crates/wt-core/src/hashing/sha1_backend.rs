//! Pluggable SHA-1 backends for piece hashing.
//!
//! Go phones use ARMv8 SHA-1 instructions. The portable `sha1` crate is soft on
//! aarch64; `ring::digest::SHA1_FOR_LEGACY_USE_ONLY` uses the hardware path on
//! both aarch64 and x86_64. Default production path is [`RingSha1`].

use sha1::{Digest, Sha1};

/// Incremental SHA-1 used while streaming piece bytes.
pub trait PieceSha1: Send {
    fn new() -> Self
    where
        Self: Sized;
    fn update(&mut self, data: &[u8]);
    fn finish(self) -> [u8; 20];
}

/// `sha1` crate (SHA-NI on x86 via cpufeatures; soft elsewhere).
pub struct SoftSha1 {
    inner: Sha1,
}

impl PieceSha1 for SoftSha1 {
    fn new() -> Self {
        Self { inner: Sha1::new() }
    }
    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
    fn finish(self) -> [u8; 20] {
        self.inner.finalize().into()
    }
}

/// `ring` SHA-1 (ARMv8 / x86 SHA extensions when available).
pub struct RingSha1 {
    inner: ring::digest::Context,
}

impl PieceSha1 for RingSha1 {
    fn new() -> Self {
        Self {
            inner: ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY),
        }
    }
    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
    fn finish(self) -> [u8; 20] {
        let dig = self.inner.finish();
        let mut out = [0u8; 20];
        out.copy_from_slice(dig.as_ref());
        out
    }
}

/// Which backend the process selected for piece hashing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sha1BackendKind {
    Soft,
    Ring,
}

fn select_backend() -> Sha1BackendKind {
    // Phones (aarch64) must use ring's ARMv8 SHA path — soft is much slower there.
    #[cfg(target_arch = "aarch64")]
    {
        return Sha1BackendKind::Ring;
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        // Microbench ~8 MiB, 1 worker; pick the faster implementation.
        const N: usize = 8 * 1024 * 1024;
        let soft = throughput_mib_s::<SoftSha1>(N, 1);
        let ring = throughput_mib_s::<RingSha1>(N, 1);
        if ring >= soft {
            Sha1BackendKind::Ring
        } else {
            Sha1BackendKind::Soft
        }
    }
}

/// Selected backend (cached). On aarch64 always ring; elsewhere the faster of soft/ring.
pub fn selected_backend() -> Sha1BackendKind {
    use std::sync::OnceLock;
    static KIND: OnceLock<Sha1BackendKind> = OnceLock::new();
    *KIND.get_or_init(select_backend)
}

/// One-shot hash with optional zero padding (hybrid v1 piece alignment).
pub fn hash_v1_with_pad<H: PieceSha1>(data: &[u8], zero_pad: usize) -> [u8; 20] {
    let mut h = H::new();
    h.update(data);
    if zero_pad > 0 {
        const ZERO_CHUNK: [u8; 4096] = [0u8; 4096];
        let mut left = zero_pad;
        while left > 0 {
            let n = left.min(ZERO_CHUNK.len());
            h.update(&ZERO_CHUNK[..n]);
            left -= n;
        }
    }
    h.finish()
}

/// Hash with the process-selected default backend.
pub fn hash_v1_default(data: &[u8], zero_pad: usize) -> [u8; 20] {
    match selected_backend() {
        Sha1BackendKind::Soft => hash_v1_with_pad::<SoftSha1>(data, zero_pad),
        Sha1BackendKind::Ring => hash_v1_with_pad::<RingSha1>(data, zero_pad),
    }
}

/// Benchmark `workers` parallel SHA-1 passes over `payload_bytes`. Returns MiB/s.
pub fn throughput_mib_s<H: PieceSha1>(payload_bytes: usize, workers: usize) -> f64 {
    use std::sync::Arc;
    use std::time::Instant;

    let workers = workers.max(1);
    let payload = Arc::new(vec![0xA5u8; payload_bytes]);
    let chunk = payload_bytes / workers;
    let start = Instant::now();
    std::thread::scope(|s| {
        for w in 0..workers {
            let payload = Arc::clone(&payload);
            let begin = w * chunk;
            let end = if w + 1 == workers {
                payload_bytes
            } else {
                begin + chunk
            };
            s.spawn(move || {
                let mut h = H::new();
                h.update(&payload[begin..end]);
                let _ = h.finish();
            });
        }
    });
    let elapsed = start.elapsed().as_secs_f64().max(1e-9);
    let mib = payload_bytes as f64 / (1024.0 * 1024.0);
    mib / elapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_and_ring_agree() {
        let msg = b"piece hashing backend parity";
        let a = hash_v1_with_pad::<SoftSha1>(msg, 0);
        let b = hash_v1_with_pad::<RingSha1>(msg, 0);
        assert_eq!(a, b);
    }
}

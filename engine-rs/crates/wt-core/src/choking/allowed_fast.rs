//! BEP 6 allowed-fast set generation.
//!
//! Canonical algorithm from <https://www.bittorrent.org/beps/bep_0006.html>.
//! Test vectors match the BEP example (CacheLogic / reference implementation).
//! anacrolix/torrent receives AllowedFast but does not generate sets; we
//! implement the BEP so the Rust engine can send them.

use sha1::{Digest, Sha1};
use std::net::Ipv4Addr;

/// Generate the allowed-fast piece set for peer `ip`.
///
/// * `ip` — peer's IPv4 (externally facing / connection remote).
/// * `infohash` — 20-byte infohash.
/// * `num_pieces` — torrent piece count (`sz`); must be > 0.
/// * `k` — desired set size (BEP recommends 10).
///
/// Returns piece indices in generation order (same order as the BEP C++
/// reference). If `num_pieces == 0` or `k == 0`, returns an empty vec.
pub fn generate_allowed_fast_set(
    ip: Ipv4Addr,
    infohash: &[u8; 20],
    num_pieces: u32,
    k: usize,
) -> Vec<u32> {
    if k == 0 || num_pieces == 0 {
        return Vec::new();
    }
    // Cap k at num_pieces — cannot have more unique indices than pieces.
    let k = k.min(num_pieces as usize);

    let mut a: Vec<u32> = Vec::with_capacity(k);

    // x = htonl(ip & 0xffffff00) || infohash
    let masked = u32::from(ip) & 0xffff_ff00;
    let mut x = Vec::with_capacity(24);
    x.extend_from_slice(&masked.to_be_bytes());
    x.extend_from_slice(infohash);

    while a.len() < k {
        let digest = Sha1::digest(&x);
        x = digest.to_vec();
        for i in 0..5 {
            if a.len() >= k {
                break;
            }
            let j = i * 4;
            let y = u32::from_be_bytes([x[j], x[j + 1], x[j + 2], x[j + 3]]);
            let index = y % num_pieces;
            if !a.contains(&index) {
                a.push(index);
            }
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BEP 6 example vectors (IP 80.4.4.200, infohash 0xaa…aa, 1313 pieces).
    #[test]
    fn bep6_example_k7() {
        let ip = Ipv4Addr::new(80, 4, 4, 200);
        let infohash = [0xaa_u8; 20];
        let set = generate_allowed_fast_set(ip, &infohash, 1313, 7);
        assert_eq!(set, vec![1059, 431, 808, 1217, 287, 376, 1188]);
    }

    #[test]
    fn bep6_example_k9() {
        let ip = Ipv4Addr::new(80, 4, 4, 200);
        let infohash = [0xaa_u8; 20];
        let set = generate_allowed_fast_set(ip, &infohash, 1313, 9);
        assert_eq!(set, vec![1059, 431, 808, 1217, 287, 376, 1188, 353, 508]);
    }

    #[test]
    fn empty_when_no_pieces() {
        assert!(generate_allowed_fast_set(Ipv4Addr::LOCALHOST, &[0; 20], 0, 10).is_empty());
    }

    #[test]
    fn k_capped_at_num_pieces() {
        let set = generate_allowed_fast_set(Ipv4Addr::new(1, 2, 3, 4), &[1; 20], 3, 10);
        assert_eq!(set.len(), 3);
        let mut sorted = set.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }
}

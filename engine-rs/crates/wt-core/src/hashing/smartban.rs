//! Smart ban cache: blame peers that sent blocks differing from verified data.
//!
//! Port of `engine-go/patches/torrent/smartban` + `smartban.go` blockCheckingWriter.

use std::collections::HashMap;
use std::hash::Hasher;
use std::net::IpAddr;

use parking_lot::RwLock;
use wt_types::RequestIndex;

/// Address we can ban (Go `bannableAddr` = `netip.Addr`).
pub type BannableAddr = IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PeerAndHash {
    peer: BannableAddr,
    hash: u64,
}

/// Records which peers contributed which block hashes; used while verifying a piece.
#[derive(Default)]
pub struct SmartBanCache {
    blocks: RwLock<HashMap<RequestIndex, Vec<PeerAndHash>>>,
}

impl SmartBanCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn hash_block(data: &[u8]) -> u64 {
        // Fast non-crypto fingerprint (Go uses a Hash func; xxhash-like FNV-1a 64).
        let mut h = fnv1a64();
        h.write(data);
        h.finish()
    }

    /// Record that `peer` sent `data` for request index `key`.
    pub fn record_block(&self, peer: BannableAddr, key: RequestIndex, data: &[u8]) {
        let hash = Self::hash_block(data);
        let mut g = self.blocks.write();
        g.entry(key).or_default().push(PeerAndHash { peer, hash });
    }

    /// Peers whose recorded hash for `key` differs from `data`.
    pub fn check_block(&self, key: RequestIndex, data: &[u8]) -> Vec<BannableAddr> {
        let correct = Self::hash_block(data);
        let g = self.blocks.read();
        g.get(&key)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| e.hash != correct)
                    .map(|e| e.peer)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check every chunk of `data` starting at `block_begin` with `chunk_size`.
    pub fn check_piece_blocks(
        &self,
        block_begin: RequestIndex,
        data: &[u8],
        chunk_size: u32,
    ) -> Vec<BannableAddr> {
        if chunk_size == 0 {
            return Vec::new();
        }
        let mut bad = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut off = 0usize;
        let mut idx = block_begin;
        while off < data.len() {
            let end = (off + chunk_size as usize).min(data.len());
            for peer in self.check_block(idx, &data[off..end]) {
                if seen.insert(peer) {
                    bad.push(peer);
                }
            }
            off = end;
            idx += 1;
        }
        bad
    }

    pub fn forget_block_seq(&self, begin: RequestIndex, count: u32) {
        let mut g = self.blocks.write();
        if g.is_empty() {
            return;
        }
        for i in 0..count {
            g.remove(&(begin + i));
        }
    }

    pub fn has_peer_for_blocks(&self, begin: RequestIndex, count: u32) -> bool {
        let g = self.blocks.read();
        if g.is_empty() {
            return false;
        }
        (0..count).any(|i| g.get(&(begin + i)).is_some_and(|v| !v.is_empty()))
    }


    pub fn clear(&self) {
        self.blocks.write().clear();
    }
}

fn fnv1a64() -> impl Hasher {
    Fnv64(0xcbf29ce484222325)
}

struct Fnv64(u64);

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn blames_differing_peer() {
        let cache = SmartBanCache::new();
        let good_peer = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let bad_peer = IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2));
        let key = 10u32;
        cache.record_block(good_peer, key, b"correct-block-data");
        cache.record_block(bad_peer, key, b"WRONG-block-data!!");
        let bad = cache.check_block(key, b"correct-block-data");
        assert_eq!(bad, vec![bad_peer]);
        assert!(cache.has_peer_for_blocks(key, 1));
        cache.forget_block_seq(key, 1);
        assert!(!cache.has_peer_for_blocks(key, 1));
    }
}

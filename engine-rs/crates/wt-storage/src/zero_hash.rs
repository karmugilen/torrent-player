use parking_lot::Mutex;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::LazyLock;

static ZERO_HASH_CACHE: LazyLock<Mutex<HashMap<i64, [u8; 20]>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static ZERO_CHUNK: [u8; 32 * 1024] = [0; 32 * 1024];

/// Computes (or returns cached) SHA-1 of `length` zero bytes.
pub fn zero_piece_hash(length: i64) -> [u8; 20] {
    assert!(length >= 0, "negative zero hash length");
    {
        let cache = ZERO_HASH_CACHE.lock();
        if let Some(&hash) = cache.get(&length) {
            return hash;
        }
    }

    let mut hasher = Sha1::new();
    let mut remaining = length as u64;
    while remaining > 0 {
        let chunk = remaining.min(ZERO_CHUNK.len() as u64) as usize;
        hasher.update(&ZERO_CHUNK[..chunk]);
        remaining -= chunk as u64;
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);

    let mut cache = ZERO_HASH_CACHE.lock();
    cache.insert(length, out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_piece_hash() {
        let h1 = zero_piece_hash(0);
        let hasher = Sha1::new();
        let d = hasher.finalize();
        assert_eq!(h1, d.as_slice());

        let h2 = zero_piece_hash(16384);
        let mut hasher2 = Sha1::new();
        hasher2.update([0u8; 16384]);
        assert_eq!(h2, hasher2.finalize().as_slice());

        // Cache hit
        assert_eq!(zero_piece_hash(16384), h2);
    }
}

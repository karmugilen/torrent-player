//! Sync piece hashing: v1 SHA-1 and v2 merkle (via `wt_meta::merkle`).

use wt_types::{PieceStorage, StorageError};

use super::sha1_backend::{
    hash_v1_default, selected_backend, throughput_mib_s, RingSha1, SoftSha1,
};
use super::smartban::{BannableAddr, SmartBanCache};

/// Which hash to verify against (mirrors Go `piece.hash` / `hashV2` / file root).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectedHash {
    /// Metainfo `pieces` SHA-1.
    V1([u8; 20]),
    /// BEP 52 piece-layer hash: merkle root padded to full `piece_length`.
    V2PieceLayer([u8; 32]),
    /// Whole-file pieces root when the file fits in one piece (no piece-length pad).
    V2FileRoot([u8; 32]),
    /// Hybrid (BEP 52 + BEP 3) piece: the bytes must pass the v1 SHA-1 (over
    /// the data plus `v1_zero_pad` zeros) **and** the v2 Merkle hash. `v2` is
    /// [`ExpectedHash::V2PieceLayer`] or [`ExpectedHash::V2FileRoot`] semantics
    /// depending on `v2_file_root`. Either failing fails the piece; a split
    /// verdict means the torrent's v1 and v2 hashes disagree and is logged.
    Hybrid {
        v1: [u8; 20],
        v2: [u8; 32],
        v2_file_root: bool,
    },
}

impl ExpectedHash {
    /// Combine an indexed v2 expectation with the piece's v1 SHA-1, if any.
    ///
    /// Hybrid torrents must check both (libtorrent fails the torrent on a
    /// v1/v2 disagreement). A v1 expectation, or `None`, is returned as is.
    pub fn with_v1(self, v1: Option<[u8; 20]>) -> Self {
        match (self, v1) {
            (ExpectedHash::V2PieceLayer(v2), Some(v1)) => ExpectedHash::Hybrid {
                v1,
                v2,
                v2_file_root: false,
            },
            (ExpectedHash::V2FileRoot(v2), Some(v1)) => ExpectedHash::Hybrid {
                v1,
                v2,
                v2_file_root: true,
            },
            (other, _) => other,
        }
    }

    /// True when the hash covers v1 SHA-1, so the caller must pass
    /// `Info::v1_zero_pad` as `v1_zero_pad`.
    pub fn uses_v1(&self) -> bool {
        matches!(self, ExpectedHash::V1(_) | ExpectedHash::Hybrid { .. })
    }

    /// The v2 part, as a pure v2 expectation.
    pub fn v2_part(&self) -> Option<ExpectedHash> {
        match *self {
            ExpectedHash::V1(_) => None,
            ExpectedHash::V2PieceLayer(_) | ExpectedHash::V2FileRoot(_) => Some(*self),
            ExpectedHash::Hybrid {
                v2, v2_file_root, ..
            } => Some(if v2_file_root {
                ExpectedHash::V2FileRoot(v2)
            } else {
                ExpectedHash::V2PieceLayer(v2)
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HashWorkError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("short read: got {got} want {want}")]
    ShortRead { got: usize, want: usize },
    #[error("cancelled")]
    Cancelled,
}

/// Result of a sync hash (without pool metadata).
#[derive(Clone, Debug)]
pub struct SyncHashOutcome {
    pub correct: bool,
    pub bytes_hashed: u64,
    pub differing_peers: Vec<BannableAddr>,
}

/// Hash one piece from `storage` into `buf` (resized as needed; reused by callers).
///
/// `v1_zero_pad` appends that many zero bytes to the v1 hasher (hybrid piece-aligned files).
/// `smartban` when present checks recorded blocks against the data being hashed.
pub fn hash_piece_sync(
    storage: &dyn PieceStorage,
    expected: &ExpectedHash,
    data_length: i64,
    piece_length: i64,
    v1_zero_pad: i64,
    smartban: Option<(
        &SmartBanCache,
        u32, /* block_begin */
        u32, /* chunk_size */
    )>,
    buf: &mut Vec<u8>,
) -> Result<SyncHashOutcome, HashWorkError> {
    let want = data_length.max(0) as usize;
    if buf.len() < want {
        buf.resize(want, 0);
    }
    let mut read = 0usize;
    while read < want {
        let n = storage.read_at(&mut buf[read..want], read as i64)?;
        if n == 0 {
            break;
        }
        read += n;
    }
    if read != want {
        return Err(HashWorkError::ShortRead { got: read, want });
    }

    let data = &buf[..want];
    let mut differing_peers = Vec::new();
    if let Some((cache, block_begin, chunk_size)) = smartban {
        if chunk_size > 0 && cache.has_peer_for_blocks(block_begin, chunk_count(want, chunk_size)) {
            differing_peers = cache.check_piece_blocks(block_begin, data, chunk_size);
        }
    }

    let v1_ok = |exp: &[u8; 20]| {
        let pad = v1_zero_pad.max(0) as usize;
        (hash_v1_default(data, pad) == *exp, (want + pad) as u64)
    };
    let v2_ok = |exp: &[u8; 32], file_root: bool| {
        let mut h = wt_meta::merkle::Hash::new();
        h.write(data);
        let sum = if file_root {
            h.sum()
        } else {
            h.sum_min_length(piece_length.max(0) as usize)
        };
        sum == *exp
    };
    let (correct, bytes_hashed) = match expected {
        ExpectedHash::V1(exp) => v1_ok(exp),
        ExpectedHash::V2PieceLayer(exp) => (v2_ok(exp, false), want as u64),
        ExpectedHash::V2FileRoot(exp) => (v2_ok(exp, true), want as u64),
        ExpectedHash::Hybrid {
            v1,
            v2,
            v2_file_root,
        } => {
            let ok2 = v2_ok(v2, *v2_file_root);
            let (ok1, bytes) = v1_ok(v1);
            if ok1 != ok2 {
                tracing::warn!(
                    v1_ok = ok1,
                    v2_ok = ok2,
                    "hybrid piece: v1 and v2 hashes disagree; failing the piece"
                );
            }
            (ok1 && ok2, bytes + want as u64)
        }
    };

    Ok(SyncHashOutcome {
        correct,
        bytes_hashed,
        differing_peers,
    })
}

fn chunk_count(len: usize, chunk_size: u32) -> u32 {
    if chunk_size == 0 {
        return 0;
    }
    (len as u32).div_ceil(chunk_size)
}

/// Measure selected SHA-1 throughput; also prints soft vs ring comparison.
pub fn sha1_throughput_mib_s(payload_bytes: usize, workers: usize) -> f64 {
    let soft = throughput_mib_s::<SoftSha1>(payload_bytes, workers);
    let ring = throughput_mib_s::<RingSha1>(payload_bytes, workers);
    let kind = selected_backend();
    let chosen = match kind {
        super::sha1_backend::Sha1BackendKind::Soft => soft,
        super::sha1_backend::Sha1BackendKind::Ring => ring,
    };
    eprintln!(
        "hashing throughput: soft={soft:.1} MiB/s ring={ring:.1} MiB/s chosen={kind:?}={chosen:.1} MiB/s (payload={} MiB, {workers} workers)",
        payload_bytes / (1024 * 1024)
    );
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_adapter::MemoryPieceStorage;
    use sha1::{Digest, Sha1};
    use std::sync::Arc;
    use wt_types::{Piece, PieceStorage};

    fn filled_piece(len: i64, fill: u8) -> Arc<MemoryPieceStorage> {
        let p = Piece {
            index: 0,
            offset: 0,
            length: len,
        };
        let s = Arc::new(MemoryPieceStorage::new(p, None));
        let data = vec![fill; len as usize];
        s.write_at(&data, 0).unwrap();
        s
    }

    #[test]
    fn v1_correct_and_corrupt() {
        let data = b"hello piece data!!";
        let mut h = Sha1::new();
        h.update(data);
        let good: [u8; 20] = h.finalize().into();

        let storage = filled_piece(data.len() as i64, 0);
        storage.write_at(data, 0).unwrap();
        let mut buf = Vec::new();
        let ok = hash_piece_sync(
            &*storage,
            &ExpectedHash::V1(good),
            data.len() as i64,
            data.len() as i64,
            0,
            None,
            &mut buf,
        )
        .unwrap();
        assert!(ok.correct);

        let mut bad = good;
        bad[0] ^= 0xff;
        let bad_r = hash_piece_sync(
            &*storage,
            &ExpectedHash::V1(bad),
            data.len() as i64,
            data.len() as i64,
            0,
            None,
            &mut buf,
        )
        .unwrap();
        assert!(!bad_r.correct);
    }

    #[test]
    fn v2_piece_layer_and_file_root() {
        // 16 KiB of data → one merkle leaf.
        let len = wt_meta::merkle::BLOCK_SIZE;
        let storage = filled_piece(len as i64, 0x3c);
        let mut data = vec![0u8; len];
        storage.read_at(&mut data, 0).unwrap();

        let mut mh = wt_meta::merkle::Hash::new();
        mh.write(&data);
        let root = mh.sum();
        let layer = mh.sum_min_length(len * 2); // pad to 2 leaves (32 KiB piece length)

        let mut buf = Vec::new();
        let r1 = hash_piece_sync(
            &*storage,
            &ExpectedHash::V2FileRoot(root),
            len as i64,
            len as i64,
            0,
            None,
            &mut buf,
        )
        .unwrap();
        assert!(r1.correct);

        let r2 = hash_piece_sync(
            &*storage,
            &ExpectedHash::V2PieceLayer(layer),
            len as i64,
            (len * 2) as i64,
            0,
            None,
            &mut buf,
        )
        .unwrap();
        assert!(r2.correct);
    }
}

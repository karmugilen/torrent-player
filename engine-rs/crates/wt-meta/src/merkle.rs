//! BitTorrent v2 Merkle trees (BEP 52).
//!
//! Port of `engine-go/patches/torrent/merkle`.

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use wt_types::FileInfo;

/// Leaf block size for BitTorrent v2 Merkle trees (16 KiB).
pub const BLOCK_SIZE: usize = 1 << 14;

/// Compute the Merkle root of a power-of-two layer of 32-byte hashes.
///
/// `hashes.len()` must be 0, 1, or a power of two. Returns an error otherwise
/// (Go panics; we return `Err` for fuzz robustness).
pub fn root(hashes: &[[u8; 32]]) -> Result<[u8; 32]> {
    match hashes.len() {
        0 => Ok(Sha256::digest([]).into()),
        1 => Ok(hashes[0]),
        n => {
            if n != round_up_to_power_of_two(n) {
                return Err(Error::Merkle(format!(
                    "expected power of two number of hashes, got {n}"
                )));
            }
            let mut next = Vec::with_capacity(n / 2);
            for i in (0..n).step_by(2) {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&hashes[i]);
                buf[32..].copy_from_slice(&hashes[i + 1]);
                next.push(Sha256::digest(buf).into());
            }
            root(&next)
        }
    }
}

/// Pad `hashes` with `pad_hash` up to the next power of two, then take the root.
pub fn root_with_pad_hash(hashes: &[[u8; 32]], pad_hash: [u8; 32]) -> Result<[u8; 32]> {
    if hashes.is_empty() {
        return root(&[]);
    }
    let target = round_up_to_power_of_two(hashes.len());
    if hashes.len() == target {
        return root(hashes);
    }
    let mut padded = hashes.to_vec();
    while padded.len() < target {
        padded.push(pad_hash);
    }
    root(&padded)
}

/// Split a compact concatenated layer into 32-byte hashes.
pub fn compact_layer_to_slice_hashes(compact: &[u8]) -> Result<Vec<[u8; 32]>> {
    let (chunks, remainder) = compact.as_chunks::<32>();
    if !remainder.is_empty() {
        return Err(Error::Merkle(format!(
            "compact layer length {} is not a multiple of 32",
            compact.len()
        )));
    }
    Ok(chunks.to_vec())
}

pub fn round_up_to_power_of_two(n: usize) -> usize {
    if n <= 1 {
        return n;
    }
    1usize << (usize::BITS - (n - 1).leading_zeros())
}

/// Incremental v2 piece/file hasher (16 KiB leaf blocks).
#[derive(Clone)]
pub struct Hash {
    blocks: Vec<[u8; 32]>,
    next_block: Sha256,
    next_block_written: usize,
}

impl Default for Hash {
    fn default() -> Self {
        Self::new()
    }
}

impl Hash {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_block: Sha256::new(),
            next_block_written: 0,
        }
    }

    fn remaining(&self) -> usize {
        BLOCK_SIZE - self.next_block_written
    }

    pub fn write(&mut self, mut p: &[u8]) {
        while !p.is_empty() {
            let take = p.len().min(self.remaining());
            self.next_block.update(&p[..take]);
            self.next_block_written += take;
            p = &p[take..];
            if self.remaining() == 0 {
                let sum: [u8; 32] = self.next_block.finalize_reset().into();
                self.blocks.push(sum);
                self.next_block_written = 0;
            }
        }
    }

    fn next_block_sum(&self) -> [u8; 32] {
        self.next_block.clone().finalize().into()
    }

    fn cur_blocks(&self) -> Vec<[u8; 32]> {
        let mut blocks = self.blocks.clone();
        if self.next_block_written != 0 {
            blocks.push(self.next_block_sum());
        }
        blocks
    }

    /// Root hash, padding missing blocks with zero hashes to a power of two.
    pub fn sum(&self) -> [u8; 32] {
        root_with_pad_hash(&self.cur_blocks(), [0u8; 32]).expect("pad to power of two")
    }

    /// Like `sum`, but first extend with zero leaf hashes so the covered length
    /// is at least `length` bytes (needed for piece-layer tail blocks).
    pub fn sum_min_length(&self, length: usize) -> [u8; 32] {
        let mut blocks = self.cur_blocks();
        let min_blocks = length.div_ceil(BLOCK_SIZE);
        while blocks.len() < min_blocks {
            blocks.push([0u8; 32]);
        }
        root_with_pad_hash(&blocks, [0u8; 32]).expect("pad to power of two")
    }

    pub fn reset(&mut self) {
        self.blocks.clear();
        self.next_block = Sha256::new();
        self.next_block_written = 0;
    }
}

/// Padding hash for the hash layer corresponding to a piece (BEP 52).
pub fn hash_for_piece_pad(piece_length: i64) -> [u8; 32] {
    let blocks_per_piece = (piece_length / BLOCK_SIZE as i64).max(0) as usize;
    let block_hashes = vec![[0u8; 32]; blocks_per_piece];
    root(&block_hashes).unwrap_or([0u8; 32])
}

/// Verify a single piece's leaf hashes against a pieces-root using the piece
/// layer pad hash.
pub fn verify_piece_layer(
    piece_layer_hash: [u8; 32],
    pieces_root: [u8; 32],
    all_layer_hashes: &[[u8; 32]],
    piece_length: i64,
) -> Result<bool> {
    let _ = piece_layer_hash;
    let root_hash = root_with_pad_hash(all_layer_hashes, hash_for_piece_pad(piece_length))?;
    Ok(root_hash == pieces_root)
}

/// Validate `piece_layers` against a v2 file tree already flattened into `files`.
///
/// `piece_layers` maps pieces-root -> concatenated layer hashes.
pub fn validate_piece_layers(
    piece_layers: &[([u8; 32], Vec<u8>)],
    files: &[FileInfo],
    piece_length: i64,
) -> Result<()> {
    let layer_map: std::collections::HashMap<[u8; 32], &Vec<u8>> =
        piece_layers.iter().map(|(k, v)| (*k, v)).collect();

    for fi in files {
        if fi.is_padding() {
            continue;
        }
        let Some(pieces_root) = fi.pieces_root else {
            continue;
        };
        match layer_map.get(&pieces_root) {
            None => {
                if fi.length > piece_length {
                    return Err(Error::Merkle(format!(
                        "no piece layers for file {:?}",
                        fi.path
                    )));
                }
            }
            Some(compact) => {
                let layer_hashes = compact_layer_to_slice_hashes(compact)?;
                let computed = root_with_pad_hash(&layer_hashes, hash_for_piece_pad(piece_length))?;
                if computed != pieces_root {
                    return Err(Error::Merkle(format!(
                        "file {:?}: expected hash {:x?} got {:x?}",
                        fi.path, pieces_root, computed
                    )));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_empty() {
        let h = root(&[]).unwrap();
        assert_eq!(h, Sha256::digest([]).as_slice());
    }

    #[test]
    fn root_one() {
        let leaf = Sha256::digest(b"abc").into();
        assert_eq!(root(&[leaf]).unwrap(), leaf);
    }

    #[test]
    fn round_up() {
        assert_eq!(round_up_to_power_of_two(0), 0);
        assert_eq!(round_up_to_power_of_two(1), 1);
        assert_eq!(round_up_to_power_of_two(2), 2);
        assert_eq!(round_up_to_power_of_two(3), 4);
        assert_eq!(round_up_to_power_of_two(5), 8);
    }

    #[test]
    fn hash_writes() {
        let mut h = Hash::new();
        h.write(&[0u8; BLOCK_SIZE]);
        h.write(&[1u8; 100]);
        let sum = h.sum();
        assert_ne!(sum, [0u8; 32]);
    }
}

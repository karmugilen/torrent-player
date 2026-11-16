//! BEP 52 block-level (16 KiB leaf) verification inside one piece.
//!
//! Metainfo `piece layers` only carry one hash per piece. To check a single
//! block before its piece is whole we need that piece's leaf hashes: they are
//! fetched from a v2 peer with a `HashRequest` at `base_layer = 0` spanning
//! exactly one piece (or one single-piece file), and trusted only when their
//! Merkle root equals the hash we already hold for the piece
//! ([`ExpectedHash::V2PieceLayer`] / [`ExpectedHash::V2FileRoot`]). No proof
//! layers are needed, because that hash is itself the proof.

use sha2::{Digest, Sha256};
use wt_meta::merkle::{root, round_up_to_power_of_two, BLOCK_SIZE};
use wt_types::{FileInfo, Info, PieceIndex};

use super::hash_exchange::{file_by_pieces_root, HashRequestKey, MAX_HASH_REQUEST_LENGTH};
use super::ExpectedHash;

/// Outcome of checking one received block against its leaf hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeafCheck {
    /// No leaf hashes for this piece (v1, layer not fetched, single-block
    /// piece): the block waits for whole-piece verification as before.
    Unknown,
    /// The block matches its Merkle leaf.
    Verified,
    /// The block does not match its Merkle leaf: the sender sent bad data.
    Bad,
}

/// SHA-256 of one block: a v2 Merkle leaf. A short final block is hashed as
/// is, without padding.
pub fn leaf_hash(block: &[u8]) -> [u8; 32] {
    Sha256::digest(block).into()
}

/// Leaves per full piece; `None` unless the piece length is a power-of-two
/// number (at least 2) of 16 KiB blocks that fits one hash request.
fn leaves_per_piece(piece_length: i64) -> Option<u32> {
    if piece_length <= 0 || piece_length % BLOCK_SIZE as i64 != 0 {
        return None;
    }
    let n = (piece_length / BLOCK_SIZE as i64) as u64;
    (n >= 2 && n.is_power_of_two() && n <= u64::from(MAX_HASH_REQUEST_LENGTH)).then_some(n as u32)
}

fn file_for_piece(info: &Info, piece: PieceIndex) -> Option<&FileInfo> {
    info.files.iter().filter(|f| !f.is_padding()).find(|f| {
        f.begin_piece_index(info.piece_length) <= piece
            && piece < f.end_piece_index(info.piece_length)
    })
}

/// The leaf span `(index, length)` in the file's leaf layer that hashes up to
/// `expected` for `piece`.
fn leaf_span(info: &Info, file: &FileInfo, piece: PieceIndex, expected: &ExpectedHash) -> Option<(u32, u32)> {
    let per_piece = leaves_per_piece(info.piece_length)?;
    match expected.v2_part()? {
        ExpectedHash::V1(_) | ExpectedHash::Hybrid { .. } => None,
        ExpectedHash::V2PieceLayer(_) => {
            let in_file = piece.checked_sub(file.begin_piece_index(info.piece_length))?;
            Some((in_file.checked_mul(per_piece)?, per_piece))
        }
        // A file within one piece: its root covers its own leaves, padded to
        // a power of two, not a whole piece.
        ExpectedHash::V2FileRoot(_) => {
            let leaves = (file.length.max(0) as usize).div_ceil(BLOCK_SIZE);
            (leaves >= 2).then(|| (0, round_up_to_power_of_two(leaves) as u32))
        }
    }
}

/// The `HashRequest` that fetches the leaf hashes of `piece`, when block-level
/// verification is possible for it.
pub fn leaf_request_for_piece(
    info: &Info,
    expected_v2: &[Option<ExpectedHash>],
    piece: PieceIndex,
) -> Option<HashRequestKey> {
    if !info.has_v2() {
        return None;
    }
    let expected = expected_v2.get(piece as usize).copied().flatten()?;
    let file = file_for_piece(info, piece)?;
    let (index, length) = leaf_span(info, file, piece, &expected)?;
    Some(HashRequestKey {
        pieces_root: file.pieces_root?,
        base_layer: 0,
        index,
        length,
        proof_layers: 0,
    })
}

/// The piece a `base_layer = 0` span `(index, length)` of the file
/// `pieces_root` covers, when it is exactly the span
/// [`leaf_request_for_piece`] asks for.
pub fn piece_for_leaf_span(
    info: &Info,
    expected_v2: &[Option<ExpectedHash>],
    pieces_root: [u8; 32],
    index: u32,
    length: u32,
) -> Option<PieceIndex> {
    let file = file_by_pieces_root(info, &pieces_root)?;
    let per_piece = leaves_per_piece(info.piece_length)?;
    let piece = file.begin_piece_index(info.piece_length).checked_add(index / per_piece)?;
    if piece >= file.end_piece_index(info.piece_length) {
        return None;
    }
    let expected = expected_v2.get(piece as usize).copied().flatten()?;
    (leaf_span(info, file, piece, &expected)? == (index, length)).then_some(piece)
}

/// True when `leaves` are the leaf layer under `expected`. Only then may they
/// be used to verify blocks.
pub fn leaf_layer_matches(expected: &ExpectedHash, leaves: &[[u8; 32]]) -> bool {
    let want = match expected.v2_part() {
        Some(ExpectedHash::V2PieceLayer(h) | ExpectedHash::V2FileRoot(h)) => h,
        _ => return false,
    };
    leaves.len() >= 2 && root(leaves).is_ok_and(|r| r == want)
}

/// Check the block at byte offset `begin` of its piece against `leaves`.
pub fn check_block(leaves: &[[u8; 32]], begin: u32, block: &[u8]) -> LeafCheck {
    if begin as usize % BLOCK_SIZE != 0 || block.is_empty() || block.len() > BLOCK_SIZE {
        return LeafCheck::Unknown;
    }
    match leaves.get(begin as usize / BLOCK_SIZE) {
        None => LeafCheck::Unknown,
        Some(want) if leaf_hash(block) == *want => LeafCheck::Verified,
        Some(_) => LeafCheck::Bad,
    }
}

/// Leaf layer of `data` (one piece, or one single-piece file), zero-padded to
/// `length` hashes as BEP 52 requires. Used to answer leaf `HashRequest`s.
pub fn leaf_layer_of(data: &[u8], length: usize) -> Vec<[u8; 32]> {
    let mut out: Vec<[u8; 32]> = data.chunks(BLOCK_SIZE).map(leaf_hash).collect();
    out.resize(length.max(out.len()), [0u8; 32]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_meta::merkle::{hash_for_piece_pad, root_with_pad_hash, Hash};

    /// One file of `len` bytes with `piece_length`; returns info, the indexed
    /// expected hashes and the file bytes.
    fn synth(piece_length: usize, len: usize) -> (Info, Vec<Option<ExpectedHash>>, Vec<u8>) {
        let data: Vec<u8> = (0..len).map(|i| (i / 7 + i / BLOCK_SIZE * 31) as u8).collect();
        let layer: Vec<[u8; 32]> = data
            .chunks(piece_length)
            .map(|p| {
                let mut h = Hash::new();
                h.write(p);
                h.sum_min_length(piece_length)
            })
            .collect();
        let (root, layers) = if layer.len() > 1 {
            let root = root_with_pad_hash(&layer, hash_for_piece_pad(piece_length as i64)).unwrap();
            (root, vec![(root, layer.concat())])
        } else {
            let mut h = Hash::new();
            h.write(&data);
            (h.sum(), Vec::new())
        };
        let info = Info {
            name: "leaf".into(),
            piece_length: piece_length as i64,
            pieces: Vec::new(),
            files: vec![FileInfo {
                path: vec!["f.bin".into()],
                length: len as i64,
                torrent_offset: 0,
                attr: String::new(),
                pieces_root: Some(root),
            }],
            single_file: true,
            private: None,
            source: String::new(),
            meta_version: 2,
            file_tree_raw: None,
        };
        let expected = crate::hashing::build_piece_layer_index(&info, &layers);
        (info, expected, data)
    }

    #[test]
    fn one_leaf_inside_a_multi_block_piece_verifies_and_corruption_is_rejected() {
        // 4 blocks per piece; 2 pieces, the last one short (2.5 blocks).
        let plen = BLOCK_SIZE * 4;
        let (info, expected, data) = synth(plen, plen + BLOCK_SIZE * 2 + BLOCK_SIZE / 2);

        let key = leaf_request_for_piece(&info, &expected, 0).expect("piece 0 leaf request");
        assert_eq!((key.base_layer, key.index, key.length, key.proof_layers), (0, 0, 4, 0));
        let leaves = leaf_layer_of(&data[..plen], key.length as usize);
        assert!(leaf_layer_matches(&expected[0].unwrap(), &leaves));

        // Block 2 alone, with no other block of the piece present.
        let block = &data[2 * BLOCK_SIZE..3 * BLOCK_SIZE];
        assert_eq!(check_block(&leaves, 2 * BLOCK_SIZE as u32, block), LeafCheck::Verified);
        let mut bad = block.to_vec();
        bad[100] ^= 1;
        assert_eq!(check_block(&leaves, 2 * BLOCK_SIZE as u32, &bad), LeafCheck::Bad);
        // Right bytes at the wrong offset are also bad.
        assert_eq!(check_block(&leaves, 0, block), LeafCheck::Bad);

        // A forged leaf layer never becomes trusted.
        let mut forged = leaves.clone();
        forged[2] = leaf_hash(&bad);
        assert!(!leaf_layer_matches(&expected[0].unwrap(), &forged));
        // Nor does piece 1's layer stand in for piece 0's.
        let leaves1 = leaf_layer_of(&data[plen..], 4);
        assert!(!leaf_layer_matches(&expected[0].unwrap(), &leaves1));
    }

    #[test]
    fn short_last_piece_pads_with_zero_leaves() {
        let plen = BLOCK_SIZE * 4;
        let (info, expected, data) = synth(plen, plen + BLOCK_SIZE * 2 + BLOCK_SIZE / 2);
        let key = leaf_request_for_piece(&info, &expected, 1).unwrap();
        assert_eq!((key.index, key.length), (4, 4));
        assert_eq!(piece_for_leaf_span(&info, &expected, key.pieces_root, 4, 4), Some(1));
        let leaves = leaf_layer_of(&data[plen..], 4);
        assert_eq!(leaves[3], [0u8; 32]);
        assert!(leaf_layer_matches(&expected[1].unwrap(), &leaves));
        // The half block at the end is a leaf of its own.
        let tail = &data[plen + 2 * BLOCK_SIZE..];
        assert_eq!(check_block(&leaves, 2 * BLOCK_SIZE as u32, tail), LeafCheck::Verified);
    }

    #[test]
    fn single_piece_file_uses_its_own_leaf_count() {
        // 3 blocks in an 8-block piece: the file root spans 4 leaves.
        let (info, expected, data) = synth(BLOCK_SIZE * 8, BLOCK_SIZE * 3);
        assert!(matches!(expected[0], Some(ExpectedHash::V2FileRoot(_))));
        let key = leaf_request_for_piece(&info, &expected, 0).unwrap();
        assert_eq!((key.index, key.length), (0, 4));
        let leaves = leaf_layer_of(&data, 4);
        assert!(leaf_layer_matches(&expected[0].unwrap(), &leaves));
        assert_eq!(piece_for_leaf_span(&info, &expected, key.pieces_root, 0, 4), Some(0));
        assert_eq!(piece_for_leaf_span(&info, &expected, key.pieces_root, 0, 8), None);
    }

    #[test]
    fn no_leaf_request_without_a_v2_expected_hash() {
        let plen = BLOCK_SIZE * 4;
        let (info, expected, _) = synth(plen, plen * 2);
        let none = vec![None; expected.len()];
        assert!(leaf_request_for_piece(&info, &none, 0).is_none());
        // Single-block pieces gain nothing: the piece is the leaf.
        let (info, expected, _) = synth(BLOCK_SIZE, BLOCK_SIZE * 3);
        assert!(leaf_request_for_piece(&info, &expected, 0).is_none());
    }
}

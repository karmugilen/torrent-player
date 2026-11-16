//! BEP 52 hash exchange helpers (outbound `HashRequest` / inbound `Hashes`).
//!
//! Parity with Go `peerconn.go` `requestMissingHashes` / `onReadHashes` /
//! `onHashRequest` / `getHashes`.

use sha2::{Digest, Sha256};
use wt_meta::merkle::{log2_rounding_up, round_up_to_power_of_two, root_with_pad_hash, BLOCK_SIZE};
use wt_types::{FileInfo, Info, PieceIndex};

use super::ExpectedHash;

/// BEP 52 max hashes in one request (Go rejects `Length > 8192`).
pub const MAX_HASH_REQUEST_LENGTH: u32 = 8192;
/// Chunk size when requesting a file's piece layer (Go uses 512).
pub const HASH_REQUEST_CHUNK: u32 = 512;
/// Cap unanswered outbound hash requests per peer (DoS / write-buffer safety).
pub const MAX_OUTSTANDING_HASH_REQUESTS: usize = 64;

/// Key for deduplicating outbound `HashRequest`s (Go `hashRequest`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HashRequestKey {
    pub pieces_root: [u8; 32],
    pub base_layer: u32,
    pub index: u32,
    pub length: u32,
    pub proof_layers: u32,
}

/// A multi-piece v2 file that still needs piece-layer hashes from peers.
#[derive(Clone, Debug)]
pub struct MissingLayerFile {
    pub pieces_root: [u8; 32],
    pub begin_piece: PieceIndex,
    pub num_pieces: u32,
}

/// `base_layer` for HashRequest given torrent piece length (Go formula).
pub fn base_layer_for_piece_length(piece_length: i64) -> u32 {
    if piece_length <= 0 {
        return 0;
    }
    // `div_ceil` on i64: `piece_length` comes from an untrusted info dict and
    // `piece_length + BLOCK_SIZE - 1` overflows near `i64::MAX`.
    let blocks = (piece_length as u64).div_ceil(BLOCK_SIZE as u64);
    if blocks > usize::MAX as u64 {
        return 64;
    }
    log2_rounding_up(round_up_to_power_of_two((blocks as usize).max(1)))
}

/// BEP 52 piece-layer pad hash: root of a subtree of `piece_length / 16 KiB`
/// zero leaf hashes. Same value as `wt_meta::merkle::hash_for_piece_pad`, but
/// computed in `O(log n)` hashes without allocating one hash per block, so an
/// absurd `piece length` from a torrent cannot make every inbound
/// `HashRequest` allocate gigabytes.
pub fn piece_pad_hash(piece_length: i64) -> [u8; 32] {
    let blocks = (piece_length / BLOCK_SIZE as i64).max(0) as u64;
    match blocks {
        0 => Sha256::digest([]).into(),
        n if !n.is_power_of_two() => [0u8; 32], // wt-meta: root() errs → zeros
        n => {
            let mut h = [0u8; 32];
            for _ in 0..n.trailing_zeros() {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&h);
                buf[32..].copy_from_slice(&h);
                h = Sha256::digest(buf).into();
            }
            h
        }
    }
}

/// Multi-piece v2 files whose `expected_v2` slots are still incomplete.
pub fn missing_layer_files(
    info: &Info,
    expected_v2: &[Option<ExpectedHash>],
) -> Vec<MissingLayerFile> {
    if !info.has_v2() {
        return Vec::new();
    }
    let piece_length = info.piece_length;
    let mut out = Vec::new();
    for fi in info.files.iter().filter(|f| !f.is_padding()) {
        let Some(pieces_root) = fi.pieces_root else {
            continue;
        };
        let begin = fi.begin_piece_index(piece_length);
        let n = fi.num_pieces(piece_length);
        if n <= 1 {
            continue;
        }
        let mut missing = false;
        for i in 0..n {
            let pi = (begin + i) as usize;
            if expected_v2.get(pi).and_then(|e| e.as_ref()).is_none() {
                missing = true;
                break;
            }
        }
        if missing {
            out.push(MissingLayerFile {
                pieces_root,
                begin_piece: begin,
                num_pieces: n,
            });
        }
    }
    out
}

/// Build outbound HashRequest keys for one file (power-of-two lengths ≤ 512).
pub fn hash_requests_for_file(
    pieces_root: [u8; 32],
    file_num_pieces: u32,
    piece_length: i64,
) -> Vec<HashRequestKey> {
    if file_num_pieces <= 1 {
        return Vec::new();
    }
    let base_layer = base_layer_for_piece_length(piece_length);
    let mut out = Vec::new();
    let mut index = 0u32;
    while index < file_num_pieces {
        let remaining = file_num_pieces - index;
        let chunk = remaining.min(HASH_REQUEST_CHUNK) as usize;
        let length = round_up_to_power_of_two(chunk).max(2) as u32;
        out.push(HashRequestKey {
            pieces_root,
            base_layer,
            index,
            length,
            proof_layers: 0,
        });
        index = index.saturating_add(length);
        if length == 0 {
            break;
        }
    }
    out
}

/// Find a non-padding file by its pieces root.
pub fn file_by_pieces_root<'a>(info: &'a Info, root: &[u8; 32]) -> Option<&'a FileInfo> {
    info.files
        .iter()
        .find(|f| !f.is_padding() && f.pieces_root.as_ref() == Some(root))
}

/// Validate a full (or partial) piece-layer buffer against `pieces_root`.
///
/// `file_hashes` must cover every file piece (padding slots beyond the file use
/// the piece pad hash when building the Merkle root). Returns true when the
/// computed root matches.
pub fn hashes_match_pieces_root(
    file_hashes: &[[u8; 32]],
    pieces_root: [u8; 32],
    piece_length: i64,
) -> bool {
    match root_with_pad_hash(file_hashes, piece_pad_hash(piece_length)) {
        Ok(root) => root == pieces_root,
        Err(_) => false,
    }
}

/// Merge inbound `Hashes` into a pending per-file buffer, then validate.
///
/// `pending` is a file-piece-length vector (Go `receivedHashPieces`). On a
/// matching Merkle root, copies into `expected` and returns `Ok(true)`. A
/// mismatch or still-incomplete buffer returns `Ok(false)` without touching
/// `expected`. Protocol errors return `Err`.
///
/// `hashes` must have the shape of a reply to a request we send
/// ([`hash_requests_for_file`], `proof_layers = 0`): a power-of-two count
/// (or one trimmed at the file's last piece), `index` a multiple of the
/// rounded count (BEP 52), inside the file's power-of-two piece layer. Anything else is rejected before it touches
/// `pending`, so a reply carrying extra (proof) hashes or a misaligned span
/// cannot overwrite hashes another reply already filled in.
pub fn apply_inbound_hashes(
    info: &Info,
    expected: &mut [Option<ExpectedHash>],
    pending: &mut Vec<[u8; 32]>,
    pieces_root: [u8; 32],
    index: u32,
    hashes: &[[u8; 32]],
) -> Result<bool, String> {
    let Some(file) = file_by_pieces_root(info, &pieces_root) else {
        return Err(format!("no file for pieces root {}", hex::encode(pieces_root)));
    };
    let piece_length = info.piece_length;
    let file_pieces = file.num_pieces(piece_length) as usize;
    if file_pieces <= 1 {
        return Err("hash exchange not used for single-piece files".into());
    }
    let n = hashes.len();
    let span = round_up_to_power_of_two(n.max(2));
    let start = index as usize;
    // Power-of-two count (as requested), or one trimmed exactly at the file's
    // last piece (pad hashes omitted). Anything else — e.g. a reply with proof
    // hashes appended — would write hashes into the wrong slots.
    let shape_ok = n.is_power_of_two() || start + n == file_pieces;
    if n == 0 || !shape_ok || span > MAX_HASH_REQUEST_LENGTH as usize {
        return Err(format!("bad Hashes count {n} at index {index}"));
    }
    if start % span != 0 || start + span > round_up_to_power_of_two(file_pieces) {
        return Err(format!("bad Hashes span index={index} count={n} file_pieces={file_pieces}"));
    }
    if pending.len() != file_pieces {
        *pending = vec![[0u8; 32]; file_pieces];
    }
    // Seed gaps from already-trusted expected hashes (e.g. metainfo layers).
    let begin = file.begin_piece_index(piece_length) as usize;
    for (i, slot) in pending.iter_mut().enumerate() {
        if *slot == [0u8; 32] {
            if let Some(ExpectedHash::V2PieceLayer(h)) = expected.get(begin + i).copied().flatten()
            {
                *slot = h;
            }
        }
    }
    for (i, h) in hashes.iter().enumerate() {
        let fi = index as usize + i;
        if fi < pending.len() {
            pending[fi] = *h;
        }
        // Hashes beyond the file are request-length pad hashes; ignore.
    }

    if !hashes_match_pieces_root(pending, pieces_root, piece_length) {
        return Ok(false);
    }

    for (i, h) in pending.iter().enumerate() {
        let pi = begin + i;
        if pi < expected.len() {
            expected[pi] = Some(ExpectedHash::V2PieceLayer(*h));
        }
    }
    Ok(true)
}

/// SHA-256 of a left/right pair (one Merkle parent).
fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(left);
    buf[32..].copy_from_slice(right);
    Sha256::digest(buf).into()
}

/// Uncle hashes for a base-layer span, lowest layer first (BEP 52).
///
/// The first `log2(length) - 1` proof layers are implicit in reducing the
/// base hashes and are omitted from the message, but still count toward
/// `proof_layers`. Remaining layers walk sibling nodes up toward the root.
///
/// `length` may be 1 when walking from a single node of an upper layer
/// (leaf proofs reuse this after reducing one piece's leaves to its piece
/// hash). Wire requests still require `length >= 2`.
pub fn proof_uncles(
    padded_layer: &[[u8; 32]],
    index: u32,
    length: u32,
    proof_layers: u32,
) -> Result<Vec<[u8; 32]>, String> {
    if proof_layers == 0 {
        return Ok(Vec::new());
    }
    let len = length as usize;
    let idx = index as usize;
    if len == 0 || !len.is_power_of_two() || idx % len != 0 || idx + len > padded_layer.len() {
        return Err("invalid proof span".into());
    }
    // Layers covered by folding the base hashes themselves.
    let omitted = length.trailing_zeros().saturating_sub(1);
    let want = proof_layers.saturating_sub(omitted) as usize;
    if want == 0 {
        return Ok(Vec::new());
    }

    // Build successive parent layers until we have enough for the walk.
    let mut layer: Vec<[u8; 32]> = padded_layer.to_vec();
    let mut layers: Vec<Vec<[u8; 32]>> = vec![layer.clone()];
    while layer.len() > 1 {
        layer = layer
            .chunks_exact(2)
            .map(|p| hash_pair(&p[0], &p[1]))
            .collect();
        layers.push(layer.clone());
    }

    let mut uncles = Vec::with_capacity(want);
    let mut pos = idx / len;
    let mut level = length.trailing_zeros() as usize;
    while uncles.len() < want {
        if level + 1 >= layers.len() {
            break;
        }
        let sibling = pos ^ 1;
        let row = &layers[level];
        if sibling >= row.len() {
            return Err("proof uncle out of range".into());
        }
        uncles.push(row[sibling]);
        pos /= 2;
        level += 1;
    }
    if uncles.len() != want {
        return Err(format!(
            "need {want} proof uncles, tree only has {}",
            uncles.len()
        ));
    }
    Ok(uncles)
}

/// Build a `Hashes` payload for an inbound request, or explain why to reject.
///
/// When `proof_layers > 0`, appends BEP 52 uncle hashes after the base span so
/// a peer (e.g. libtorrent) can verify the span against `pieces_root` without
/// already holding the rest of the layer. Rejects malformed requests instead
/// of answering with a wrongly shaped proof.
pub fn serve_hashes(
    info: &Info,
    expected: &[Option<ExpectedHash>],
    pieces_root: [u8; 32],
    index: u32,
    length: u32,
    proof_layers: u32,
) -> Result<Vec<[u8; 32]>, String> {
    // BEP 52 shape: power-of-two length in 2..=8192, index a multiple of it.
    wt_wire::validate_hash_request(0, index, length, proof_layers).map_err(str::to_string)?;
    let Some(file) = file_by_pieces_root(info, &pieces_root) else {
        return Err(format!("no file for pieces root {}", hex::encode(pieces_root)));
    };
    let piece_length = info.piece_length;
    let begin = file.begin_piece_index(piece_length) as usize;
    let end = file.end_piece_index(piece_length) as usize;
    let file_span = end.saturating_sub(begin);
    let rounded = round_up_to_power_of_two(file_span.max(1));
    if index as usize + length as usize > rounded {
        return Err("invalid hash range".into());
    }
    let pad = piece_pad_hash(piece_length);
    // Full padded piece layer: needed both for base hashes past the file end
    // and for building uncle proofs up to the pieces root.
    let mut padded = Vec::with_capacity(rounded);
    for i in 0..rounded {
        let torrent_piece = begin + i;
        if torrent_piece >= end {
            padded.push(pad);
            continue;
        }
        match expected.get(torrent_piece).copied().flatten() {
            Some(ExpectedHash::V2PieceLayer(h)) => padded.push(h),
            Some(ExpectedHash::V2FileRoot(h)) if file_span <= 1 => padded.push(h),
            _ => {
                return Err(format!("missing piece layer hash for piece {torrent_piece}"));
            }
        }
    }
    let start = index as usize;
    let end_span = start + length as usize;
    let mut out = padded[start..end_span].to_vec();
    if proof_layers > 0 {
        out.extend(proof_uncles(&padded, index, length, proof_layers)?);
    }
    Ok(out)
}

/// [`serve_hashes`] for a request that names its `base_layer`: only the
/// piece layer (`base_layer_for_piece_length`) is served from the piece-layer
/// index; any other layer is rejected instead of answered with hashes of the
/// wrong layer.
pub fn serve_hashes_at_layer(
    info: &Info,
    expected: &[Option<ExpectedHash>],
    pieces_root: [u8; 32],
    base_layer: u32,
    index: u32,
    length: u32,
    proof_layers: u32,
) -> Result<Vec<[u8; 32]>, String> {
    let want = base_layer_for_piece_length(info.piece_length);
    if base_layer != want {
        return Err(format!("base layer {base_layer} is not the piece layer {want}"));
    }
    serve_hashes(info, expected, pieces_root, index, length, proof_layers)
}

/// Compact piece-layer bytes for persistence after a successful exchange.
pub fn compact_layer_from_expected(
    info: &Info,
    expected: &[Option<ExpectedHash>],
    pieces_root: [u8; 32],
) -> Option<Vec<u8>> {
    let file = file_by_pieces_root(info, &pieces_root)?;
    let piece_length = info.piece_length;
    let begin = file.begin_piece_index(piece_length) as usize;
    let n = file.num_pieces(piece_length) as usize;
    if n <= 1 {
        return None;
    }
    let mut compact = Vec::with_capacity(n * 32);
    for i in 0..n {
        match expected.get(begin + i).copied().flatten() {
            Some(ExpectedHash::V2PieceLayer(h)) => compact.extend_from_slice(&h),
            _ => return None,
        }
    }
    Some(compact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_meta::{parse_torrent, validate_metainfo_piece_layers};

    use crate::hashing::build_piece_layer_index;

    #[test]
    fn base_layer_for_256kib_piece() {
        // 256 KiB / 16 KiB = 16 blocks → log2(16) = 4
        assert_eq!(base_layer_for_piece_length(256 * 1024), 4);
    }

    #[test]
    fn hybrid_fixture_serve_and_round_trip() {
        let bytes = include_bytes!("../../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
        let (mi, info) = parse_torrent(bytes).unwrap();
        validate_metainfo_piece_layers(&mi, &info).unwrap();
        let expected = build_piece_layer_index(&info, &mi.piece_layers);
        assert!(missing_layer_files(&info, &expected).is_empty());

        let darkroom = info
            .files
            .iter()
            .find(|f| f.path.iter().any(|p| p.contains("Darkroom")))
            .unwrap();
        let root = darkroom.pieces_root.unwrap();
        let n = darkroom.num_pieces(info.piece_length);
        let served = serve_hashes(&info, &expected, root, 0, n.next_power_of_two().max(2), 0)
            .expect("serve");
        assert!(served.len() >= n as usize);

        let mut empty = vec![None; info.num_pieces() as usize];
        let mut pending = Vec::new();
        let applied =
            apply_inbound_hashes(&info, &mut empty, &mut pending, root, 0, &served[..n as usize])
                .expect("apply");
        assert!(applied);
        for i in 0..n as usize {
            assert_eq!(empty[i], expected[i]);
        }
    }

    #[test]
    fn corrupt_hashes_rejected() {
        let bytes = include_bytes!("../../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
        let (mi, info) = parse_torrent(bytes).unwrap();
        let expected = build_piece_layer_index(&info, &mi.piece_layers);
        let darkroom = info
            .files
            .iter()
            .find(|f| f.path.iter().any(|p| p.contains("Darkroom")))
            .unwrap();
        let root = darkroom.pieces_root.unwrap();
        let n = darkroom.num_pieces(info.piece_length) as usize;
        let mut bad: Vec<[u8; 32]> = (0..n)
            .map(|i| match expected[i] {
                Some(ExpectedHash::V2PieceLayer(h)) => h,
                _ => [0u8; 32],
            })
            .collect();
        bad[0][0] ^= 0xff;
        let mut empty = vec![None; info.num_pieces() as usize];
        let mut pending = Vec::new();
        let ok = apply_inbound_hashes(&info, &mut empty, &mut pending, root, 0, &bad).unwrap();
        assert!(!ok, "corrupt layer must not validate");
        assert!(empty.iter().all(|e| e.is_none()), "corrupt must not commit");
    }

    #[test]
    fn request_keys_chunk_power_of_two() {
        let keys = hash_requests_for_file([1u8; 32], 13, 256 * 1024);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].index, 0);
        assert_eq!(keys[0].length, 16); // round_up(13) = 16
        assert_eq!(keys[0].proof_layers, 0);
    }

    #[test]
    fn serve_hashes_with_proofs_verifies_against_pieces_root() {
        use wt_meta::merkle::verify_hashes_with_proof;

        let bytes = include_bytes!("../../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
        let (mi, info) = parse_torrent(bytes).unwrap();
        validate_metainfo_piece_layers(&mi, &info).unwrap();
        let expected = build_piece_layer_index(&info, &mi.piece_layers);
        let darkroom = info
            .files
            .iter()
            .find(|f| f.path.iter().any(|p| p.contains("Darkroom")))
            .unwrap();
        let root = darkroom.pieces_root.unwrap();
        let n = darkroom.num_pieces(info.piece_length) as u64; // 13 → padded 16
        // Half the padded layer: (16/8).trailing_zeros() = 1 uncle to the root.
        // omitted = log2(8)-1 = 2, so proof_layers = 2 + 1 = 3.
        let length = 8u32;
        let proof_layers = 3u32;
        let served = serve_hashes(&info, &expected, root, 0, length, proof_layers).expect("serve");
        assert_eq!(served.len(), length as usize + 1);
        let (base, uncles) = served.split_at(length as usize);
        assert!(verify_hashes_with_proof(&root, 0, base, uncles, n));
        // Only the omitted layers (proof_layers == log2(length)-1): no uncles sent.
        let bare_proof = serve_hashes(&info, &expected, root, 0, length, 2).expect("omitted only");
        assert_eq!(bare_proof.len(), length as usize);
        // Wrong base must not verify.
        let mut bad = served.clone();
        bad[0][0] ^= 1;
        let (b, u) = bad.split_at(length as usize);
        assert!(!verify_hashes_with_proof(&root, 0, b, u, n));
        // Misaligned / non-power-of-two requests are rejected before answering.
        assert!(serve_hashes(&info, &expected, root, 1, 2, 0).is_err());
        assert!(serve_hashes(&info, &expected, root, 0, 3, 0).is_err());
        assert!(serve_hashes(&info, &expected, root, 0, 8, 64).is_err());
        // More proof layers than the tree has → reject, not a truncated proof.
        assert!(serve_hashes(&info, &expected, root, 0, 8, 20).is_err());
    }

    #[test]
    fn serve_hashes_at_layer_rejects_wrong_base_layer() {
        let bytes = include_bytes!("../../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
        let (mi, info) = parse_torrent(bytes).unwrap();
        let expected = build_piece_layer_index(&info, &mi.piece_layers);
        let root = info.files[0].pieces_root.unwrap();
        let want = base_layer_for_piece_length(info.piece_length);
        assert!(serve_hashes_at_layer(&info, &expected, root, want, 0, 16, 0).is_ok());
        assert!(serve_hashes_at_layer(&info, &expected, root, 0, 0, 16, 0).is_err());
        assert!(serve_hashes_at_layer(&info, &expected, root, want + 1, 0, 16, 0).is_err());
    }
}

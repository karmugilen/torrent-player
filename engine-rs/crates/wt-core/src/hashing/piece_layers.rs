//! Map global piece index → v2 expected hash from metainfo `piece layers`.
//!
//! Parity with Go `Torrent.addPieceLayersLocked` / `Piece.hasPieceLayer`.

use wt_meta::merkle::{compact_layer_to_slice_hashes, root_with_pad_hash};
use wt_types::{Info, PieceIndex};

use super::hash_exchange::piece_pad_hash;
use super::ExpectedHash;

/// Build a per-piece v2 expected-hash table from `info` + `piece_layers`.
///
/// Missing layers for multi-piece files soft-fail: those slots stay `None` and
/// a warning is logged. Single-piece files use [`ExpectedHash::V2FileRoot`].
pub fn build_piece_layer_index(
    info: &Info,
    piece_layers: &[([u8; 32], Vec<u8>)],
) -> Vec<Option<ExpectedHash>> {
    let num = info.num_pieces() as usize;
    let mut out = vec![None; num];
    if !info.has_v2() || num == 0 {
        return out;
    }

    let layer_map: std::collections::HashMap<[u8; 32], &Vec<u8>> =
        piece_layers.iter().map(|(k, v)| (*k, v)).collect();
    let piece_length = info.piece_length;

    for fi in info.files.iter().filter(|f| !f.is_padding()) {
        let Some(pieces_root) = fi.pieces_root else {
            continue;
        };
        let begin = fi.begin_piece_index(piece_length) as usize;
        let file_pieces = fi.num_pieces(piece_length) as usize;
        if file_pieces == 0 {
            continue;
        }

        // Go `hasPieceLayer`: only files larger than one piece use piece layers.
        if file_pieces <= 1 {
            if begin < out.len() {
                out[begin] = Some(ExpectedHash::V2FileRoot(pieces_root));
            }
            continue;
        }

        let Some(compact) = layer_map.get(&pieces_root) else {
            tracing::warn!(
                path = ?fi.path,
                "no piece layers for multi-piece v2 file; leaving pieces without V2 expected hash"
            );
            continue;
        };

        let hashes = match compact_layer_to_slice_hashes(compact) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(path = ?fi.path, error = %e, "bad piece layers; skipping file");
                continue;
            }
        };
        if hashes.len() != file_pieces {
            tracing::warn!(
                path = ?fi.path,
                got = hashes.len(),
                expected = file_pieces,
                "piece layer count mismatch; skipping file"
            );
            continue;
        }

        // Optional integrity check (same as Go); soft-fail on mismatch.
        match root_with_pad_hash(&hashes, piece_pad_hash(piece_length)) {
            Ok(root) if root == pieces_root => {}
            Ok(_) => {
                tracing::warn!(
                    path = ?fi.path,
                    "piece layer root mismatch; skipping file"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(path = ?fi.path, error = %e, "piece layer root error; skipping file");
                continue;
            }
        }

        for (i, hash) in hashes.into_iter().enumerate() {
            let pi = begin + i;
            if pi < out.len() {
                out[pi] = Some(ExpectedHash::V2PieceLayer(hash));
            }
        }
    }

    out
}

/// Look up the indexed v2 expected hash for `piece`, if any.
pub fn expected_v2_for_piece(
    index: &[Option<ExpectedHash>],
    piece: PieceIndex,
) -> Option<ExpectedHash> {
    index.get(piece as usize).copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_meta::{parse_torrent, validate_metainfo_piece_layers};

    #[test]
    fn hybrid_fixture_layers_resolve_sample_pieces() {
        let bytes = include_bytes!("../../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
        let (mi, info) = parse_torrent(bytes).expect("parse hybrid");
        assert!(info.has_v1() && info.has_v2());
        validate_metainfo_piece_layers(&mi, &info).expect("layers valid");

        let index = build_piece_layer_index(&info, &mi.piece_layers);
        assert_eq!(index.len(), info.num_pieces() as usize);
        assert_eq!(info.num_pieces(), (info.pieces.len() / 20) as u32);

        // First file Darkroom: 13 piece-layer hashes at pieces 0..12.
        let darkroom = info
            .files
            .iter()
            .find(|f| f.path.iter().any(|p| p.contains("Darkroom")))
            .expect("Darkroom file");
        let begin = darkroom.begin_piece_index(info.piece_length) as usize;
        assert_eq!(begin, 0);
        assert_eq!(darkroom.num_pieces(info.piece_length), 13);

        let layer = mi
            .piece_layers
            .iter()
            .find(|(root, _)| Some(*root) == darkroom.pieces_root)
            .expect("Darkroom layer");
        let hashes = compact_layer_to_slice_hashes(&layer.1).unwrap();
        assert_eq!(hashes.len(), 13);

        match index[0] {
            Some(ExpectedHash::V2PieceLayer(h)) => assert_eq!(h, hashes[0]),
            other => panic!("piece 0 expected V2PieceLayer, got {other:?}"),
        }
        match index[12] {
            Some(ExpectedHash::V2PieceLayer(h)) => assert_eq!(h, hashes[12]),
            other => panic!("piece 12 expected V2PieceLayer, got {other:?}"),
        }

        // readme.txt is 61 bytes → single piece → V2FileRoot.
        let readme = info
            .files
            .iter()
            .find(|f| f.path.iter().any(|p| p == "readme.txt"))
            .expect("readme");
        let ri = readme.begin_piece_index(info.piece_length) as usize;
        assert_eq!(readme.num_pieces(info.piece_length), 1);
        match index[ri] {
            Some(ExpectedHash::V2FileRoot(root)) => {
                assert_eq!(Some(root), readme.pieces_root);
            }
            other => panic!("readme piece expected V2FileRoot, got {other:?}"),
        }
    }

    #[test]
    fn classic_v1_index_is_empty() {
        let bytes = include_bytes!("../../../wt-meta/testdata/trackerless.torrent");
        let (mi, info) = parse_torrent(bytes).unwrap();
        assert!(info.has_v1() && !info.has_v2());
        let index = build_piece_layer_index(&info, &mi.piece_layers);
        assert!(index.iter().all(|e| e.is_none()));
    }
}

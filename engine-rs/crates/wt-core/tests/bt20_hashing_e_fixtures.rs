//! Area E: last piece of a multi-piece file and files smaller than one piece
//! verify correctly against the real libtorrent BEP 52 fixtures.

use std::sync::Arc;

use wt_core::hashing::{
    build_piece_layer_index, hash_piece_sync, serve_hashes, serve_hashes_at_layer,
    ExpectedHash,
};
use wt_core::storage_adapter::MemoryPieceStorage;
use wt_core::PieceStorage;
use wt_meta::merkle::{verify_hashes_with_proof, BLOCK_SIZE};
use wt_meta::{parse_torrent, validate_metainfo_piece_layers};
use wt_types::Piece;

fn mem_piece(index: u32, data: &[u8]) -> Arc<MemoryPieceStorage> {
    let s = Arc::new(MemoryPieceStorage::new(
        Piece {
            index,
            offset: 0,
            length: data.len() as i64,
        },
        None,
    ));
    s.write_at(data, 0).unwrap();
    s
}

fn verify(expected: ExpectedHash, data_len: i64, piece_length: i64, v1_pad: i64, data: &[u8]) -> bool {
    let storage = mem_piece(0, data);
    let mut buf = Vec::new();
    hash_piece_sync(
        &*storage,
        &expected,
        data_len,
        piece_length,
        v1_pad,
        None,
        &mut buf,
    )
    .unwrap()
    .correct
}

#[test]
fn fixture_single_piece_files_use_file_root_and_reject_junk() {
    for name in [
        "bittorrent-v2-test.torrent",
        "bittorrent-v2-hybrid-test.torrent",
    ] {
        let path = format!("{}/../wt-meta/testdata/{name}", env!("CARGO_MANIFEST_DIR"));
        let (mi, info) = parse_torrent(&std::fs::read(&path).unwrap()).unwrap();
        validate_metainfo_piece_layers(&mi, &info).unwrap();
        let index = build_piece_layer_index(&info, &mi.piece_layers);
        let mut checked = 0;
        for fi in info.files.iter().filter(|f| !f.is_padding()) {
            if fi.num_pieces(info.piece_length) != 1 {
                continue;
            }
            let pi = fi.begin_piece_index(info.piece_length) as usize;
            let Some(ExpectedHash::V2FileRoot(root)) = index[pi] else {
                panic!("{name}: {:?} expected V2FileRoot", fi.path);
            };
            assert_eq!(Some(root), fi.pieces_root);
            // We do not ship payload bytes; junk must fail the file root.
            let junk = vec![0u8; fi.length.max(1) as usize];
            assert!(
                !verify(
                    ExpectedHash::V2FileRoot(root),
                    fi.length,
                    info.piece_length,
                    0,
                    &junk
                ),
                "{name}: junk matched root for {:?}",
                fi.path
            );
            checked += 1;
        }
        assert!(checked > 0, "{name}: expected at least one single-piece file");
    }
}

#[test]
fn fixture_last_piece_of_multi_piece_file_is_shorter_and_indexed() {
    let path = format!(
        "{}/../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent",
        env!("CARGO_MANIFEST_DIR")
    );
    let (mi, info) = parse_torrent(&std::fs::read(path).unwrap()).unwrap();
    validate_metainfo_piece_layers(&mi, &info).unwrap();
    let index = build_piece_layer_index(&info, &mi.piece_layers);

    let darkroom = info
        .files
        .iter()
        .find(|f| f.path.iter().any(|p| p.contains("Darkroom")))
        .expect("Darkroom");
    let begin = darkroom.begin_piece_index(info.piece_length);
    let n = darkroom.num_pieces(info.piece_length);
    assert!(n > 1);
    let last = begin + n - 1;
    let last_len = info.piece_data_length(last);
    assert!(
        last_len > 0 && last_len < info.piece_length,
        "last piece of Darkroom should be a short tail, got {last_len}"
    );
    assert!(matches!(
        index[last as usize],
        Some(ExpectedHash::V2PieceLayer(_))
    ));
    // Hybrid: v1 pad fills the rest of the piece for the SHA-1.
    if info.has_v1() {
        assert_eq!(info.v1_zero_pad(last), info.piece_length - last_len);
        let both = index[last as usize]
            .unwrap()
            .with_v1(info.piece_hash_v1(last));
        let junk = vec![0u8; last_len as usize];
        assert!(!verify(
            both,
            last_len,
            info.piece_length,
            info.v1_zero_pad(last),
            &junk
        ));
    }
}

#[test]
fn fixture_serve_partial_layer_with_proofs_against_real_roots() {
    let path = format!(
        "{}/../wt-meta/testdata/bittorrent-v2-test.torrent",
        env!("CARGO_MANIFEST_DIR")
    );
    let (mi, info) = parse_torrent(&std::fs::read(path).unwrap()).unwrap();
    validate_metainfo_piece_layers(&mi, &info).unwrap();
    let expected = build_piece_layer_index(&info, &mi.piece_layers);
    let base = wt_core::hashing::base_layer_for_piece_length(info.piece_length);

    let mut proved = 0;
    for fi in info.files.iter().filter(|f| !f.is_padding()) {
        let n = fi.num_pieces(info.piece_length);
        if n <= 1 {
            continue;
        }
        let root = fi.pieces_root.unwrap();
        // First two piece-layer hashes + enough proof layers to reach the root.
        let length = 2u32;
        let padded = (n as usize).next_power_of_two() as u32;
        let levels_above = (padded / length).trailing_zeros();
        let omitted = length.trailing_zeros().saturating_sub(1);
        let proof_layers = omitted + levels_above;
        let served = serve_hashes_at_layer(
            &info,
            &expected,
            root,
            base,
            0,
            length,
            proof_layers,
        )
        .unwrap_or_else(|e| panic!("{:?}: {e}", fi.path));
        assert_eq!(served.len(), (length + levels_above) as usize);
        let (hashes, uncles) = served.split_at(length as usize);
        assert!(
            verify_hashes_with_proof(&root, 0, hashes, uncles, n as u64),
            "{:?} proof failed",
            fi.path
        );
        // Same request without proofs still returns only the base span.
        let bare = serve_hashes(&info, &expected, root, 0, length, 0).unwrap();
        assert_eq!(bare, hashes);
        proved += 1;
    }
    assert!(proved > 0);
    // Piece length must be a multiple of 16 KiB for the fixture.
    assert_eq!(info.piece_length % BLOCK_SIZE as i64, 0);
}

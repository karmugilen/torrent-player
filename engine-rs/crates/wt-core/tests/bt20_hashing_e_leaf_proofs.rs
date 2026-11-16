//! Area E follow-up: `serve_leaf_hashes` answers `proof_layers > 0` with
//! uncle hashes that verify to the file's pieces root (fixture geometry).

use std::sync::Arc;

use wt_core::hashing::{leaf_layer_of, ExpectedHash};
use wt_core::peer_conn::PeerTorrent;
use wt_core::storage_adapter::MemoryClientStorage;
use wt_core::torrent::TorrentInner;
use wt_meta::merkle::{
    hash_for_piece_pad, root_with_pad_hash, verify_hashes_with_proof, Hash, BLOCK_SIZE,
};
use wt_meta::{parse_torrent, validate_metainfo_piece_layers};
use wt_types::{FileInfo, Info, InfoHashV2};

fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i % 251 + i / BLOCK_SIZE as usize * 17) as u8)
        .collect()
}

/// Build a v2 torrent with the same piece length and piece count as a
/// multi-piece file from a real BEP 52 fixture, with known payload so we can
/// serve leaf hashes.
fn torrent_like_fixture_file(
    fixture: &str,
    file_substr: &str,
) -> (Arc<TorrentInner>, [u8; 32], u32, u32, u64, Vec<u8>) {
    let path = format!(
        "{}/../wt-meta/testdata/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let (mi, fix) = parse_torrent(&std::fs::read(path).unwrap()).unwrap();
    validate_metainfo_piece_layers(&mi, &fix).unwrap();
    let sample = fix
        .files
        .iter()
        .find(|f| !f.is_padding() && f.path.iter().any(|p| p.contains(file_substr)))
        .unwrap_or_else(|| panic!("{fixture}: missing {file_substr}"));
    let piece_length = fix.piece_length;
    let n = sample.num_pieces(piece_length);
    assert!(n > 1, "{fixture}: need a multi-piece file");
    let leaves_per = (piece_length / BLOCK_SIZE as i64) as u32;
    assert!(leaves_per >= 2 && leaves_per.is_power_of_two());

    let data = payload((n as usize) * piece_length as usize);
    let layer: Vec<[u8; 32]> = data
        .chunks(piece_length as usize)
        .map(|p| {
            let mut h = Hash::new();
            h.write(p);
            h.sum_min_length(piece_length as usize)
        })
        .collect();
    let root = root_with_pad_hash(&layer, hash_for_piece_pad(piece_length)).unwrap();
    let info = Arc::new(Info {
        name: "leaf-proof".into(),
        piece_length,
        pieces: Vec::new(),
        files: vec![FileInfo {
            path: vec!["f.bin".into()],
            length: data.len() as i64,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: Some(root),
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 2,
        file_tree_raw: None,
    });
    let v2 = InfoHashV2([0x51; 32]);
    let t = Arc::new(TorrentInner::new(
        v2.to_short(),
        Some(v2),
        "leaf-proof".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info_with_layers(Arc::clone(&info), &[(root, layer.concat())])
        .unwrap();
    assert!(matches!(
        t.expected_v2.read()[0],
        Some(ExpectedHash::V2PieceLayer(_))
    ));
    let file_leaves = (data.len() as u64).div_ceil(BLOCK_SIZE as u64);
    (t, root, leaves_per, n, file_leaves, data)
}

#[test]
fn hybrid_fixture_geometry_leaf_proofs_verify_to_pieces_root() {
    let (t, root, leaves_per, n_pieces, file_leaves, data) =
        torrent_like_fixture_file("bittorrent-v2-hybrid-test.torrent", "Darkroom");
    let piece_length = t.info.read().as_ref().unwrap().piece_length as usize;

    // Install leaf hashes for piece 0 the way a peer exchange would.
    let leaves = leaf_layer_of(&data[..piece_length], leaves_per as usize);
    assert!(t.apply_peer_leaf_hashes(root, 0, &leaves).unwrap());

    let padded_pieces = (n_pieces as u32).next_power_of_two();
    let levels_above = (padded_pieces / 1).trailing_zeros(); // from one piece-layer node
    // Equivalent: (file_leaves.next_power_of_two() / leaves_per).trailing_zeros()
    assert_eq!(
        levels_above,
        (file_leaves.next_power_of_two() / u64::from(leaves_per)).trailing_zeros()
    );
    let omitted = leaves_per.trailing_zeros().saturating_sub(1);
    let proof_layers = omitted + levels_above;

    let served = PeerTorrent::serve_leaf_hashes(t.as_ref(), root, 0, leaves_per, proof_layers)
        .expect("serve leaf proofs");
    assert_eq!(served.len(), leaves_per as usize + levels_above as usize);
    assert_eq!(&served[..leaves_per as usize], &leaves[..]);
    let (base, uncles) = served.split_at(leaves_per as usize);
    assert!(
        verify_hashes_with_proof(&root, 0, base, uncles, file_leaves),
        "leaf span + uncles must prove to the pieces root"
    );

    // Truncated / oversized proofs are rejected.
    assert!(PeerTorrent::serve_leaf_hashes(t.as_ref(), root, 0, leaves_per, proof_layers + 8).is_err());
    assert!(PeerTorrent::serve_leaf_hashes(t.as_ref(), root, 1, leaves_per, 0).is_err());
    assert!(PeerTorrent::serve_leaf_hashes(t.as_ref(), root, 0, 3, 0).is_err());
}

#[test]
fn pure_v2_fixture_geometry_leaf_proofs_verify_to_pieces_root() {
    let path = format!(
        "{}/../wt-meta/testdata/bittorrent-v2-test.torrent",
        env!("CARGO_MANIFEST_DIR")
    );
    let (mi, fix) = parse_torrent(&std::fs::read(path).unwrap()).unwrap();
    validate_metainfo_piece_layers(&mi, &fix).unwrap();
    // Pure v2 uses 4 MiB pieces — keep the synthetic file to two pieces so the
    // test stays light while matching the fixture piece length.
    let piece_length = fix.piece_length;
    assert_eq!(piece_length, 4 << 20);
    let leaves_per = (piece_length / BLOCK_SIZE as i64) as u32;
    let n_pieces = 2u32;
    let data = payload((n_pieces as usize) * piece_length as usize);
    let layer: Vec<[u8; 32]> = data
        .chunks(piece_length as usize)
        .map(|p| {
            let mut h = Hash::new();
            h.write(p);
            h.sum_min_length(piece_length as usize)
        })
        .collect();
    let root = root_with_pad_hash(&layer, hash_for_piece_pad(piece_length)).unwrap();
    let info = Arc::new(Info {
        name: "pure-leaf-proof".into(),
        piece_length,
        pieces: Vec::new(),
        files: vec![FileInfo {
            path: vec!["f.bin".into()],
            length: data.len() as i64,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: Some(root),
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 2,
        file_tree_raw: None,
    });
    let v2 = InfoHashV2([0x52; 32]);
    let t = Arc::new(TorrentInner::new(
        v2.to_short(),
        Some(v2),
        "pure-leaf-proof".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info_with_layers(Arc::clone(&info), &[(root, layer.concat())])
        .unwrap();

    let leaves = leaf_layer_of(&data[..piece_length as usize], leaves_per as usize);
    assert!(t.apply_peer_leaf_hashes(root, 0, &leaves).unwrap());

    let file_leaves = (data.len() as u64).div_ceil(BLOCK_SIZE as u64);
    let omitted = leaves_per.trailing_zeros().saturating_sub(1);
    let levels_above = (file_leaves.next_power_of_two() / u64::from(leaves_per)).trailing_zeros();
    let proof_layers = omitted + levels_above;
    let served =
        PeerTorrent::serve_leaf_hashes(t.as_ref(), root, 0, leaves_per, proof_layers).unwrap();
    let (base, uncles) = served.split_at(leaves_per as usize);
    assert!(verify_hashes_with_proof(&root, 0, base, uncles, file_leaves));
    // Tampered uncle must not verify.
    let mut bad = served.clone();
    *bad.last_mut().unwrap() = [0xee; 32];
    let (b, u) = bad.split_at(leaves_per as usize);
    assert!(!verify_hashes_with_proof(&root, 0, b, u, file_leaves));
}

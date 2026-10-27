//! Piece hash correct/corrupt + v1/v2 using wt-meta testdata.

use std::sync::Arc;

use sha1::{Digest, Sha1};
use wt_core::hashing::{hash_piece_sync, ExpectedHash};
use wt_core::storage_adapter::MemoryPieceStorage;
use wt_core::PieceStorage;
use wt_types::{FileInfo, Info, Piece};

fn mem_piece(index: u32, data: &[u8]) -> Arc<MemoryPieceStorage> {
    let p = Piece {
        index,
        offset: 0,
        length: data.len() as i64,
    };
    let s = Arc::new(MemoryPieceStorage::new(p, None));
    s.write_at(data, 0).unwrap();
    s
}

#[test]
fn v1_correct_and_corrupt() {
    let data = b"abcdefghijklmnopqrst";
    let mut h = Sha1::new();
    h.update(data);
    let good: [u8; 20] = h.finalize().into();
    let storage = mem_piece(0, data);
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
    bad[3] ^= 1;
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
fn v2_merkle_file_root_and_piece_layer() {
    let len = wt_meta::merkle::BLOCK_SIZE;
    let data = vec![0x5au8; len];
    let storage = mem_piece(0, &data);
    let mut mh = wt_meta::merkle::Hash::new();
    mh.write(&data);
    let root = mh.sum();
    let layer = mh.sum_min_length(len * 2);
    let mut buf = Vec::new();
    assert!(
        hash_piece_sync(
            &*storage,
            &ExpectedHash::V2FileRoot(root),
            len as i64,
            len as i64,
            0,
            None,
            &mut buf,
        )
        .unwrap()
        .correct
    );
    assert!(
        hash_piece_sync(
            &*storage,
            &ExpectedHash::V2PieceLayer(layer),
            len as i64,
            (len * 2) as i64,
            0,
            None,
            &mut buf,
        )
        .unwrap()
        .correct
    );
}

#[test]
fn v1_from_wt_meta_testdata_info() {
    // Parse a real torrent; synthesize matching piece bytes for piece 0 hash.
    let bytes = include_bytes!("../../wt-meta/testdata/trackerless.torrent");
    let (_mi, info) = wt_meta::parse_torrent(bytes).expect("parse testdata");
    assert!(info.has_v1());
    assert!(info.num_pieces() >= 1);
    let piece_len = info.piece(0).length as usize;
    // We do not have the payload; verify that hashing random data fails the expected hash.
    let junk = vec![0u8; piece_len];
    let storage = mem_piece(0, &junk);
    let expected = ExpectedHash::V1(info.piece_hash_v1(0).unwrap());
    let mut buf = Vec::new();
    let r = hash_piece_sync(
        &*storage,
        &expected,
        piece_len as i64,
        info.piece_length,
        0,
        None,
        &mut buf,
    )
    .unwrap();
    assert!(!r.correct, "random data must not match metainfo piece hash");
}

#[test]
fn v2_testdata_parses_with_piece_layers() {
    let bytes = include_bytes!("../../wt-meta/testdata/bittorrent-v2-test.torrent");
    let (mi, info) = wt_meta::parse_torrent(bytes).expect("parse v2 testdata");
    assert!(info.has_v2());
    assert!(
        !mi.piece_layers.is_empty() || info.files.iter().any(|f| f.pieces_root.is_some()),
        "v2 torrent should carry piece layers or file roots"
    );
    // Hash empty/wrong data against first available root-style expectation.
    if let Some(root) = info.files.iter().find_map(|f| f.pieces_root) {
        let flen = info
            .files
            .iter()
            .find(|f| f.pieces_root == Some(root))
            .unwrap()
            .length;
        if flen > 0 && flen <= info.piece_length {
            let junk = vec![0u8; flen as usize];
            let storage = mem_piece(0, &junk);
            let mut buf = Vec::new();
            let r = hash_piece_sync(
                &*storage,
                &ExpectedHash::V2FileRoot(root),
                flen,
                info.piece_length,
                0,
                None,
                &mut buf,
            )
            .unwrap();
            assert!(!r.correct);
        }
    }
    let _ = FileInfo::default();
    let _ = Info::default();
}

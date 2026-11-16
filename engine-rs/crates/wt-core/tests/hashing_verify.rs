//! Piece hash correct/corrupt + v1/v2 using wt-meta testdata.

use std::sync::Arc;
use std::time::Duration;

use sha1::{Digest, Sha1};
use wt_core::hashing::{
    build_piece_layer_index, hash_piece_sync, ExpectedHash, PieceHasherPool,
};
use wt_core::peer_conn::PeerTorrent;
use wt_core::storage_adapter::{MemoryClientStorage, MemoryPieceStorage};
use wt_core::torrent::TorrentInner;
use wt_core::PieceStorage;
use wt_types::{FileInfo, Info, InfoHash, Piece};

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
fn pure_v2_testdata_piece_layer_index_and_num_pieces() {
    let bytes = include_bytes!("../../wt-meta/testdata/bittorrent-v2-test.torrent");
    let (mi, info) = wt_meta::parse_torrent(bytes).expect("parse v2 testdata");
    assert!(info.has_v2());
    assert!(!info.has_v1(), "pure v2 fixture must not carry v1 pieces");
    wt_meta::validate_metainfo_piece_layers(&mi, &info).expect("validate layers");
    let index = build_piece_layer_index(&info, &mi.piece_layers);
    assert_eq!(index.len(), info.num_pieces() as usize);
    assert!(info.num_pieces() > 0);
    // Every multi-piece file slot should resolve to a V2 expected hash.
    let with_hash = index.iter().filter(|e| e.is_some()).count();
    assert!(with_hash > 0, "pure v2 must index piece-layer hashes");
    // Piece data length stays within piece_length (alignment / pad excluded).
    for i in 0..info.num_pieces() {
        let len = info.piece_data_length(i);
        assert!(len > 0 && len <= info.piece_length, "piece {i} len {len}");
    }
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

#[test]
fn hybrid_fixture_piece_layer_index_maps_v2_hashes() {
    let bytes = include_bytes!("../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
    let (mi, info) = wt_meta::parse_torrent(bytes).expect("parse hybrid");
    wt_meta::validate_metainfo_piece_layers(&mi, &info).expect("validate");
    let index = build_piece_layer_index(&info, &mi.piece_layers);
    assert_eq!(index.len(), info.num_pieces() as usize);
    assert_eq!(info.num_pieces() as usize, info.pieces.len() / 20);

    let first = index[0].expect("piece 0 has v2 hash");
    assert!(
        matches!(first, ExpectedHash::V2PieceLayer(_)),
        "first hybrid piece should be V2PieceLayer, got {first:?}"
    );

    // Single-piece readme.txt → V2FileRoot.
    let readme = info
        .files
        .iter()
        .find(|f| f.path.iter().any(|p| p == "readme.txt"))
        .expect("readme");
    let ri = readme.begin_piece_index(info.piece_length) as usize;
    match index[ri] {
        Some(ExpectedHash::V2FileRoot(root)) => assert_eq!(Some(root), readme.pieces_root),
        other => panic!("expected V2FileRoot for readme, got {other:?}"),
    }
}

fn synth_v2_two_piece() -> (Info, Vec<([u8; 32], Vec<u8>)>, Vec<u8>, Vec<u8>) {
    let piece_length = (wt_meta::merkle::BLOCK_SIZE * 2) as i64;
    let data0 = vec![0x11u8; piece_length as usize];
    let data1 = vec![0x22u8; piece_length as usize / 2];
    let file_len = data0.len() as i64 + data1.len() as i64;

    let mut h0 = wt_meta::merkle::Hash::new();
    h0.write(&data0);
    let layer0 = h0.sum_min_length(piece_length as usize);
    let mut h1 = wt_meta::merkle::Hash::new();
    h1.write(&data1);
    let layer1 = h1.sum_min_length(piece_length as usize);

    let pad = wt_meta::merkle::hash_for_piece_pad(piece_length);
    let root = wt_meta::merkle::root_with_pad_hash(&[layer0, layer1], pad).unwrap();
    let mut compact = Vec::with_capacity(64);
    compact.extend_from_slice(&layer0);
    compact.extend_from_slice(&layer1);

    // Also embed v1 hashes so hybrid prefer-V2 vs fallback-V1 is testable.
    let mut pieces = Vec::with_capacity(40);
    for data in [&data0[..], &data1[..]] {
        let mut sha = Sha1::new();
        sha.update(data);
        // Hybrid v1 pads short last piece with zeros to piece_length.
        if data.len() < piece_length as usize {
            sha.update(&vec![0u8; piece_length as usize - data.len()]);
        }
        pieces.extend_from_slice(&sha.finalize());
    }

    let info = Info {
        name: "synth".into(),
        piece_length,
        pieces,
        files: vec![FileInfo {
            path: vec!["f.bin".into()],
            length: file_len,
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
    (info, vec![(root, compact)], data0, data1)
}

#[test]
fn queue_piece_hash_prefers_v2_when_layers_present() {
    let (info, layers, data0, _data1) = synth_v2_two_piece();
    let info = Arc::new(info);
    let t = Arc::new(TorrentInner::new(
        InfoHash([0x11; 20]),
        None,
        "hybrid".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info_with_layers(Arc::clone(&info), &layers)
        .expect("set_info_with_layers");

    let expected = t.expected_v2.read()[0].expect("indexed v2 for piece 0");
    assert!(matches!(expected, ExpectedHash::V2PieceLayer(_)));

    // Write good bytes, queue, expect V2 verify success.
    {
        let storage = t.storage.read().clone().unwrap();
        storage
            .piece(info.piece(0), info.piece_hash_v1(0))
            .write_at(&data0, 0)
            .unwrap();
    }
    let (pool, rx) = PieceHasherPool::with_workers(1);
    *t.hasher.lock() = Some(pool);
    PeerTorrent::queue_piece_hash(t.as_ref(), 0);
    assert!(t.pieces.read()[0].checking, "piece should be queued for hash");
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("hash result");
    assert_eq!(result.piece_index, 0);
    assert!(result.correct, "good bytes must pass V2 piece-layer hash");
    assert!(!result.cancelled);

    // Corrupt and re-queue.
    {
        let mut pieces = t.pieces_mut();
        pieces[0].checking = false;
        pieces[0].complete = false;
    }
    {
        let storage = t.storage.read().clone().unwrap();
        let mut bad = data0.clone();
        bad[0] ^= 0xff;
        storage
            .piece(info.piece(0), info.piece_hash_v1(0))
            .write_at(&bad, 0)
            .unwrap();
    }
    PeerTorrent::queue_piece_hash(t.as_ref(), 0);
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("hash result");
    assert!(!result.correct, "corrupt bytes must fail V2 piece-layer hash");
}

#[test]
fn queue_piece_hash_v1_regression_without_layers() {
    // Small classic v1 torrent (not a multi-hundred-MB fixture).
    let piece_len = 16 * 1024i64;
    let data = vec![0x5au8; piece_len as usize];
    let mut sha = Sha1::new();
    sha.update(&data);
    let hash: [u8; 20] = sha.finalize().into();
    let info = Arc::new(Info {
        name: "v1".into(),
        piece_length: piece_len,
        pieces: hash.to_vec(),
        files: vec![FileInfo {
            path: vec![],
            length: piece_len,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 1,
        file_tree_raw: None,
    });
    let t = Arc::new(TorrentInner::new(
        InfoHash([0x22; 20]),
        None,
        "v1".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info(Arc::clone(&info)).expect("set_info");
    assert!(
        t.expected_v2.read().iter().all(|e| e.is_none()),
        "classic v1 must not index V2 expected hashes"
    );

    {
        let storage = t.storage.read().clone().unwrap();
        storage
            .piece(info.piece(0), info.piece_hash_v1(0))
            .write_at(&data, 0)
            .unwrap();
    }
    let (pool, rx) = PieceHasherPool::with_workers(1);
    *t.hasher.lock() = Some(pool);
    PeerTorrent::queue_piece_hash(t.as_ref(), 0);
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("hash result");
    assert!(result.correct, "matching v1 bytes must pass ExpectedHash::V1");

    {
        let mut pieces = t.pieces_mut();
        pieces[0].checking = false;
        pieces[0].complete = false;
    }
    {
        let storage = t.storage.read().clone().unwrap();
        let mut bad = data.clone();
        bad[0] ^= 1;
        storage
            .piece(info.piece(0), info.piece_hash_v1(0))
            .write_at(&bad, 0)
            .unwrap();
    }
    PeerTorrent::queue_piece_hash(t.as_ref(), 0);
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("hash result");
    assert!(!result.correct, "corrupt v1 bytes must fail");
}

#[test]
fn synthetic_v2_piece_layer_good_and_corrupt() {
    let (info, layers, data0, data1) = synth_v2_two_piece();
    let piece_length = info.piece_length;
    wt_meta::validate_metainfo_piece_layers(
        &wt_types::MetaInfo {
            piece_layers: layers.clone(),
            ..Default::default()
        },
        &info,
    )
    .expect("synthetic layers valid");

    let index = build_piece_layer_index(&info, &layers);
    assert_eq!(index.len(), 2);
    let layer0 = match index[0] {
        Some(ExpectedHash::V2PieceLayer(h)) => h,
        other => panic!("expected V2PieceLayer, got {other:?}"),
    };
    let layer1 = match index[1] {
        Some(ExpectedHash::V2PieceLayer(h)) => h,
        other => panic!("expected V2PieceLayer, got {other:?}"),
    };

    let mut buf = Vec::new();
    let s0 = mem_piece(0, &data0);
    assert!(
        hash_piece_sync(
            &*s0,
            &ExpectedHash::V2PieceLayer(layer0),
            data0.len() as i64,
            piece_length,
            0,
            None,
            &mut buf,
        )
        .unwrap()
        .correct
    );
    let mut corrupt = data0.clone();
    corrupt[0] ^= 0xff;
    let s_bad = mem_piece(0, &corrupt);
    assert!(
        !hash_piece_sync(
            &*s_bad,
            &ExpectedHash::V2PieceLayer(layer0),
            corrupt.len() as i64,
            piece_length,
            0,
            None,
            &mut buf,
        )
        .unwrap()
        .correct
    );

    let s1 = mem_piece(1, &data1);
    assert!(
        hash_piece_sync(
            &*s1,
            &ExpectedHash::V2PieceLayer(layer1),
            data1.len() as i64,
            piece_length,
            0,
            None,
            &mut buf,
        )
        .unwrap()
        .correct
    );
}

//! BitTorrent 2.0 hashing audit: per-file piece verification against
//! independently computed Merkle roots, hybrid v1+v2, hash exchange from a
//! magnet (no piece layers), and hostile inputs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};
use wt_core::hashing::{
    apply_inbound_hashes, base_layer_for_piece_length, build_piece_layer_index,
    compact_layer_from_expected, hash_piece_sync, hash_requests_for_file, leaf_layer_matches,
    leaf_layer_of, missing_layer_files, piece_pad_hash, serve_hashes, serve_hashes_at_layer,
    ExpectedHash,
    PieceHasherPool,
};
use wt_core::peer_conn::PeerTorrent;
use wt_core::storage_adapter::{MemoryClientStorage, MemoryPieceStorage};
use wt_core::torrent::TorrentInner;
use wt_core::PieceStorage;
use wt_meta::merkle::{compact_layer_to_slice_hashes, hash_for_piece_pad, Hash, BLOCK_SIZE};
use wt_meta::{parse_torrent, validate_metainfo_piece_layers};
use wt_types::{FileInfo, Info, InfoHash, Piece};

const PIECE: usize = 4 * BLOCK_SIZE; // 64 KiB: 4 leaves per piece

fn payload(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ seed ^ (i / BLOCK_SIZE) as u8).collect()
}

/// Whole-file BEP 52 pieces root, computed over 16 KiB leaves without going
/// through piece layers — an independent reference for the per-piece path.
fn file_root(data: &[u8]) -> [u8; 32] {
    let mut h = Hash::new();
    h.write(data);
    h.sum()
}

fn piece_layer(data: &[u8]) -> Vec<[u8; 32]> {
    data.chunks(PIECE)
        .map(|p| {
            let mut h = Hash::new();
            h.write(p);
            h.sum_min_length(PIECE)
        })
        .collect()
}

fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

struct Hybrid {
    info: Info,
    layers: Vec<([u8; 32], Vec<u8>)>,
    a: Vec<u8>,
    b: Vec<u8>,
}

/// Hybrid torrent laid out like libtorrent creates them:
/// `a` (2.5 pieces) · pad file (0.5 piece) · `b` (0.3 piece, single-piece file).
/// v1 piece 2 spans the end of `a` plus the pad zeros.
fn hybrid() -> Hybrid {
    let a = payload(2 * PIECE + PIECE / 2, 0x5a);
    let b = payload(PIECE * 3 / 10, 0xa5);
    let pad = vec![0u8; PIECE / 2];
    let mut v1_stream = a.clone();
    v1_stream.extend_from_slice(&pad);
    v1_stream.extend_from_slice(&b);
    let pieces: Vec<u8> = v1_stream.chunks(PIECE).flat_map(|p| sha1(&[p])).collect();
    let root_a = file_root(&a);
    let root_b = file_root(&b);
    let info = Info {
        name: "hybrid".into(),
        piece_length: PIECE as i64,
        pieces,
        files: vec![
            FileInfo {
                path: vec!["a.bin".into()],
                length: a.len() as i64,
                torrent_offset: 0,
                attr: String::new(),
                pieces_root: Some(root_a),
            },
            FileInfo {
                path: vec![".pad".into(), "32768".into()],
                length: pad.len() as i64,
                torrent_offset: a.len() as i64,
                attr: "p".into(),
                pieces_root: None,
            },
            FileInfo {
                path: vec!["b.bin".into()],
                length: b.len() as i64,
                torrent_offset: 3 * PIECE as i64,
                attr: String::new(),
                pieces_root: Some(root_b),
            },
        ],
        single_file: false,
        private: None,
        source: String::new(),
        meta_version: 2,
        file_tree_raw: None,
    };
    let layers = vec![(root_a, piece_layer(&a).concat())];
    Hybrid { info, layers, a, b }
}

/// File bytes of global piece `i` (v2 view: no pad bytes).
fn piece_bytes<'a>(h: &'a Hybrid, i: u32) -> &'a [u8] {
    match i {
        0..=2 => {
            let s = i as usize * PIECE;
            &h.a[s..(s + PIECE).min(h.a.len())]
        }
        3 => &h.b,
        _ => unreachable!(),
    }
}

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

fn verify(info: &Info, expected: ExpectedHash, i: u32, data: &[u8]) -> bool {
    let pad = if expected.uses_v1() { info.v1_zero_pad(i) } else { 0 };
    let storage = mem_piece(i, data);
    let mut buf = Vec::new();
    hash_piece_sync(&*storage, &expected, data.len() as i64, info.piece_length, pad, None, &mut buf)
        .unwrap()
        .correct
}

#[test]
fn bt20_hashing_hybrid_every_piece_verifies_against_independent_roots() {
    let h = hybrid();
    let info = &h.info;
    assert_eq!(info.num_pieces(), 4);
    assert_eq!(info.num_pieces() as usize, info.pieces.len() / 20, "v1/v2 piece counts agree");
    // Global piece ↔ file mapping, pad excluded from v2 data but zero-padded for v1.
    assert_eq!(info.piece(2).length, (PIECE / 2) as i64);
    assert_eq!(info.v1_zero_pad(2), (PIECE / 2) as i64);
    assert_eq!(info.v1_zero_pad(3), 0, "no trailing pad after the last file");

    let index = build_piece_layer_index(info, &h.layers);
    for i in 0..3 {
        assert!(matches!(index[i], Some(ExpectedHash::V2PieceLayer(_))), "piece {i}");
    }
    assert!(matches!(index[3], Some(ExpectedHash::V2FileRoot(_))));

    for i in 0..4u32 {
        let v2 = index[i as usize].unwrap();
        let data = piece_bytes(&h, i);
        assert!(verify(info, v2, i, data), "v2 piece {i}");
        let both = v2.with_v1(info.piece_hash_v1(i));
        assert!(matches!(both, ExpectedHash::Hybrid { .. }));
        assert!(verify(info, both, i, data), "hybrid piece {i}");
        let mut bad = data.to_vec();
        *bad.last_mut().unwrap() ^= 1;
        assert!(!verify(info, v2, i, &bad), "corrupt v2 piece {i}");
        assert!(!verify(info, both, i, &bad), "corrupt hybrid piece {i}");
    }
}

#[test]
fn bt20_hashing_hybrid_v1_v2_disagreement_fails_the_piece() {
    let h = hybrid();
    let index = build_piece_layer_index(&h.info, &h.layers);
    // Data matches v2 but the torrent's v1 hash says otherwise (inconsistent
    // hybrid): the piece must fail, whichever side disagrees.
    for i in 0..4u32 {
        let data = piece_bytes(&h, i);
        let v2 = index[i as usize].unwrap();
        let mut wrong_v1 = h.info.piece_hash_v1(i).unwrap();
        wrong_v1[0] ^= 1;
        assert!(!verify(&h.info, v2.with_v1(Some(wrong_v1)), i, data), "piece {i} v1 wrong");
        let wrong_v2 = match v2 {
            ExpectedHash::V2PieceLayer(mut x) => {
                x[0] ^= 1;
                ExpectedHash::V2PieceLayer(x)
            }
            ExpectedHash::V2FileRoot(mut x) => {
                x[0] ^= 1;
                ExpectedHash::V2FileRoot(x)
            }
            other => panic!("{other:?}"),
        };
        assert!(!verify(&h.info, wrong_v2.with_v1(h.info.piece_hash_v1(i)), i, data), "piece {i} v2 wrong");
    }
    // Leaf verification keeps working on the v2 part of a hybrid expectation.
    let both = index[0].unwrap().with_v1(h.info.piece_hash_v1(0));
    assert!(leaf_layer_matches(&both, &leaf_layer_of(&h.a[..PIECE], 4)));
}

/// Magnet flow: info without piece layers → hash exchange fills them →
/// every piece hashes correct through `TorrentInner`.
#[test]
fn bt20_hashing_magnet_hash_exchange_then_full_verify_through_torrent() {
    let h = hybrid();
    let mut pure = h.info.clone();
    pure.pieces.clear(); // pure v2: only the exchanged layer can verify `a`
    let info = Arc::new(pure);
    let t = Arc::new(TorrentInner::new(
        InfoHash([0x42; 20]),
        None,
        "magnet".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info_with_layers(Arc::clone(&info), &[]).unwrap();
    let missing = t.missing_piece_layer_files();
    assert_eq!(missing.len(), 1, "only the multi-piece file needs a layer");
    let root_a = info.files[0].pieces_root.unwrap();
    assert_eq!(missing[0].pieces_root, root_a);
    assert!(t.expected_v2.read()[0].is_none());

    // A seed serves the layer; we apply what it sends for our request.
    let seed_index = build_piece_layer_index(&info, &h.layers);
    let keys = hash_requests_for_file(root_a, missing[0].num_pieces, info.piece_length);
    assert_eq!(keys.len(), 1);
    assert_eq!((keys[0].base_layer, keys[0].index, keys[0].length), (2, 0, 4));
    let k = keys[0];
    let served =
        serve_hashes(&info, &seed_index, k.pieces_root, k.index, k.length, k.proof_layers).unwrap();
    assert_eq!(served.len(), 4, "reply carries the requested count, pad hash included");
    assert_eq!(served[3], piece_pad_hash(info.piece_length));
    // A forged reply is not trusted.
    let mut forged = served.clone();
    forged[1][5] ^= 1;
    assert!(!t.apply_peer_piece_hashes(root_a, 0, &forged).unwrap());
    assert!(t.expected_v2.read()[1].is_none());
    assert!(t.apply_peer_piece_hashes(root_a, 0, &served).unwrap());
    assert!(t.missing_piece_layer_files().is_empty());

    let storage = t.storage.read().clone().unwrap();
    for i in 0..4u32 {
        storage.piece(info.piece(i), None).write_at(piece_bytes(&h, i), 0).unwrap();
    }
    let (pool, rx) = PieceHasherPool::with_workers(1);
    *t.hasher.lock() = Some(pool);
    for i in 0..4u32 {
        PeerTorrent::queue_piece_hash(t.as_ref(), i);
        let r = rx.recv_timeout(Duration::from_secs(5)).expect("hash result");
        assert_eq!(r.piece_index, i);
        assert!(r.correct, "piece {i} must verify after the hash exchange");
    }
}

fn synth_many(n_pieces: usize) -> (Info, [u8; 32], Vec<[u8; 32]>) {
    let layer: Vec<[u8; 32]> = (0..n_pieces)
        .map(|i| {
            let mut x = [0u8; 32];
            x[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
            x[31] = 0xee;
            x
        })
        .collect();
    let root = wt_meta::merkle::root_with_pad_hash(&layer, piece_pad_hash(PIECE as i64)).unwrap();
    let info = Info {
        name: "many".into(),
        piece_length: PIECE as i64,
        pieces: Vec::new(),
        files: vec![FileInfo {
            path: vec!["big".into()],
            length: (n_pieces * PIECE - 100) as i64,
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
    (info, root, layer)
}

/// > 512 pieces: the layer arrives in several replies, in any order, and a
/// malformed reply must not corrupt a chunk that already arrived.
#[test]
fn bt20_hashing_multi_chunk_layer_and_malformed_replies() {
    let (info, root, layer) = synth_many(600);
    let full = build_piece_layer_index(&info, &[(root, layer.concat())]);
    assert!(full.iter().all(|e| matches!(e, Some(ExpectedHash::V2PieceLayer(_)))));
    let keys = hash_requests_for_file(root, 600, info.piece_length);
    assert_eq!(
        keys.iter().map(|k| (k.index, k.length)).collect::<Vec<_>>(),
        vec![(0, 512), (512, 128)]
    );
    let reply = |k: &wt_core::hashing::HashRequestKey| {
        serve_hashes(&info, &full, root, k.index, k.length, 0).unwrap()
    };
    let r0 = reply(&keys[0]);
    let r1 = reply(&keys[1]);

    let mut expected = vec![None; 600];
    let mut pending = Vec::new();
    // Second chunk first.
    assert!(!apply_inbound_hashes(&info, &mut expected, &mut pending, root, 512, &r1).unwrap());
    // A reply with trailing (proof-style) hashes or a misaligned span is
    // rejected and must not overwrite slots 512.. that already arrived.
    let mut with_uncles = r0.clone();
    with_uncles.extend([[0xab; 32], [0xcd; 32]]);
    assert!(apply_inbound_hashes(&info, &mut expected, &mut pending, root, 0, &with_uncles).is_err());
    assert!(apply_inbound_hashes(&info, &mut expected, &mut pending, root, 3, &r0[..4]).is_err());
    assert!(apply_inbound_hashes(&info, &mut expected, &mut pending, root, 1024, &r0[..4]).is_err());
    assert!(apply_inbound_hashes(&info, &mut expected, &mut pending, root, 0, &[]).is_err());
    assert!(expected.iter().all(|e| e.is_none()));
    // First chunk completes and validates the whole file.
    assert!(apply_inbound_hashes(&info, &mut expected, &mut pending, root, 0, &r0).unwrap());
    assert_eq!(expected, full);
    assert_eq!(compact_layer_from_expected(&info, &expected, root).unwrap(), layer.concat());
}

/// Both real BEP 52 fixtures: every multi-piece file's layer survives
/// serve → apply into an empty (magnet) index, and matches the metainfo.
#[test]
fn bt20_hashing_fixtures_full_layer_round_trip() {
    for name in ["bittorrent-v2-test.torrent", "bittorrent-v2-hybrid-test.torrent"] {
        let path = format!("{}/../wt-meta/testdata/{name}", env!("CARGO_MANIFEST_DIR"));
        let (mi, info) = parse_torrent(&std::fs::read(path).unwrap()).unwrap();
        validate_metainfo_piece_layers(&mi, &info).unwrap();
        if info.has_v1() {
            assert_eq!(info.num_pieces() as usize, info.pieces.len() / 20, "{name}");
        }
        let full = build_piece_layer_index(&info, &mi.piece_layers);
        assert!(missing_layer_files(&info, &full).is_empty(), "{name}");
        let mut empty = vec![None; full.len()];
        let missing = missing_layer_files(&info, &empty);
        let multi = info
            .files
            .iter()
            .filter(|f| !f.is_padding() && f.num_pieces(info.piece_length) > 1)
            .count();
        assert_eq!(missing.len(), multi, "{name}");
        assert!(multi > 0, "{name}");
        let mut pending = Vec::new();
        for m in &missing {
            pending.clear();
            let mut committed = false;
            for k in hash_requests_for_file(m.pieces_root, m.num_pieces, info.piece_length) {
                let served =
                    serve_hashes(&info, &full, k.pieces_root, k.index, k.length, 0).unwrap();
                assert_eq!(served.len(), k.length as usize);
                committed = apply_inbound_hashes(
                    &info,
                    &mut empty,
                    &mut pending,
                    m.pieces_root,
                    k.index,
                    &served,
                )
                .unwrap();
            }
            assert!(committed, "{name}: layer for {:x?} validated", &m.pieces_root[..4]);
            let compact = compact_layer_from_expected(&info, &empty, m.pieces_root).unwrap();
            let meta = &mi.piece_layers.iter().find(|(r, _)| *r == m.pieces_root).unwrap().1;
            assert_eq!(&compact, meta, "{name}");
            assert_eq!(compact_layer_to_slice_hashes(meta).unwrap().len(), m.num_pieces as usize);
        }
        // Single-piece files are verified straight against the pieces root
        // and are never asked for over the wire.
        for (i, (a, b)) in full.iter().zip(&empty).enumerate() {
            match a {
                Some(ExpectedHash::V2FileRoot(_)) => assert!(b.is_none(), "{name} piece {i}"),
                _ => assert_eq!(a, b, "{name} piece {i}"),
            }
        }
    }
}

#[test]
fn bt20_hashing_pad_hash_matches_bep52_and_is_cheap_for_absurd_piece_length() {
    let mut pl = BLOCK_SIZE as i64;
    while pl <= 64 << 20 {
        assert_eq!(piece_pad_hash(pl), hash_for_piece_pad(pl), "piece length {pl}");
        pl <<= 1;
    }
    for odd in [0, 1, 3 * BLOCK_SIZE as i64, -5] {
        assert_eq!(piece_pad_hash(odd), hash_for_piece_pad(odd), "piece length {odd}");
    }
    // wt-meta's version would allocate 2^32 * 32 B = 128 GiB here; a peer
    // can trigger it with a HashRequest for any file of such a torrent.
    let start = Instant::now();
    let huge = 1i64 << 46;
    let _ = piece_pad_hash(huge);
    let info = Info {
        name: "huge".into(),
        piece_length: huge,
        pieces: Vec::new(),
        files: vec![FileInfo {
            path: vec!["f".into()],
            length: 5,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: Some([7; 32]),
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 2,
        file_tree_raw: None,
    };
    let index = build_piece_layer_index(&info, &[]);
    let _ = serve_hashes(&info, &index, [7; 32], 0, 2, 0);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn bt20_hashing_base_layer_no_overflow_on_hostile_piece_length() {
    assert_eq!(base_layer_for_piece_length(BLOCK_SIZE as i64), 0);
    assert_eq!(base_layer_for_piece_length(PIECE as i64), 2);
    assert_eq!(base_layer_for_piece_length(16 << 20), 10);
    // Debug builds used to panic on `piece_length + BLOCK_SIZE - 1` overflow.
    let _ = base_layer_for_piece_length(i64::MAX);
    let _ = base_layer_for_piece_length(i64::MAX - 1);
}

/// Inbound HashRequest validation (BEP 52): malformed spans and other layers
/// are rejected; well-shaped proof requests are answered with uncle hashes
/// that verify against the pieces root (libtorrent asks for proofs).
#[test]
fn bt20_hashing_serve_rejects_malformed_requests() {
    use wt_meta::merkle::verify_hashes_with_proof;

    let (info, root, layer) = synth_many(600);
    let full = build_piece_layer_index(&info, &[(root, layer.concat())]);
    let ok = serve_hashes_at_layer(&info, &full, root, 2, 512, 64, 0).unwrap();
    assert_eq!(ok.len(), 64);
    assert_eq!(ok[0], layer[512]);
    // Full proof to the pieces root: padded width 1024, length 2 → 9 uncles
    // (omitted = log2(2)-1 = 0, so proof_layers = 9).
    let with_proof = serve_hashes_at_layer(&info, &full, root, 2, 0, 2, 9).unwrap();
    assert_eq!(with_proof.len(), 11);
    let (base, uncles) = with_proof.split_at(2);
    assert!(verify_hashes_with_proof(&root, 0, base, uncles, 600));
    // A shorter proof is still answered (libtorrent may hold an intermediate
    // hash); it just does not reach the root by itself.
    let partial = serve_hashes_at_layer(&info, &full, root, 2, 0, 2, 3).unwrap();
    assert_eq!(partial.len(), 5);
    for (base, index, length, proofs) in [
        (2, 0, 3, 0),      // not a power of two
        (2, 0, 1, 0),      // below BEP 52 minimum of 2
        (2, 4, 8, 0),      // index not a multiple of length
        (2, 0, 16384, 0),  // above 8192
        (2, 1024, 2, 0),   // outside the file's padded layer
        (2, 0, 2, 20),     // more proof layers than the tree has
        (2, 0, 2, 64),     // proof_layers out of validate_hash_request range
        (0, 0, 2, 0),      // leaf layer: not from the piece-layer index
        (5, 0, 2, 0),      // some other layer
    ] {
        assert!(
            serve_hashes_at_layer(&info, &full, root, base, index, length, proofs).is_err(),
            "base={base} index={index} length={length} proofs={proofs}"
        );
    }
    assert!(serve_hashes_at_layer(&info, &full, [9; 32], 2, 0, 2, 0).is_err(), "unknown root");
    let none = vec![None; full.len()];
    assert!(serve_hashes(&info, &none, root, 0, 2, 0).is_err(), "layer not known yet");
}

/// The torrent-wide pending buffer is shared by every peer. A well-shaped but
/// forged chunk from one peer blocks validation only until any honest reply
/// for that chunk arrives: nothing forged is ever committed, and the honest
/// chunk overwrites it (no reset that would discard honest chunks).
#[test]
fn bt20_hashing_shared_pending_self_heals_after_forged_chunk() {
    let (info, root, layer) = synth_many(600);
    let full = build_piece_layer_index(&info, &[(root, layer.concat())]);
    let r0 = serve_hashes(&info, &full, root, 0, 512, 0).unwrap();
    let r1 = serve_hashes(&info, &full, root, 512, 128, 0).unwrap();
    let mut forged1 = r1.clone();
    forged1[7][0] ^= 1;

    let mut expected = vec![None; 600];
    let mut pending = Vec::new();
    assert!(!apply_inbound_hashes(&info, &mut expected, &mut pending, root, 0, &r0).unwrap());
    // Malicious peer B answers chunk 1 with a forged span.
    assert!(!apply_inbound_hashes(&info, &mut expected, &mut pending, root, 512, &forged1).unwrap());
    assert!(expected.iter().all(|e| e.is_none()), "forged layer never committed");
    // Honest peer C (or a retry) answers chunk 1: chunk 0 from A is kept.
    assert!(apply_inbound_hashes(&info, &mut expected, &mut pending, root, 512, &r1).unwrap());
    assert_eq!(expected, full);
}

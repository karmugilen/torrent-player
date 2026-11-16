//! BEP 52 metadata: fixtures, Merkle math, and hostile .torrent input.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use wt_meta::bencode::{self, Value};
use wt_meta::merkle::{
    self, compact_layer_to_slice_hashes, hash_for_piece_pad, root, root_with_pad_hash,
    verify_hashes_with_proof, Hash, BLOCK_SIZE,
};
use wt_meta::{
    info_hash_v1, info_hash_v2, parse_info, parse_metainfo, parse_torrent,
    validate_metainfo_piece_layers,
};

fn load(name: &str) -> Vec<u8> {
    std::fs::read(format!("testdata/{name}")).unwrap()
}

fn d(pairs: Vec<(&str, Value)>) -> Value {
    Value::Dict(pairs.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())
}

fn b(s: &[u8]) -> Value {
    Value::Bytes(s.to_vec())
}

/// `{"": {"length": len, "pieces root": root}}`
fn file_node(len: i64, root: Option<[u8; 32]>) -> Value {
    let mut props = vec![("length", Value::Int(len))];
    if let Some(r) = root {
        props.push(("pieces root", b(&r)));
    }
    d(vec![("", d(props))])
}

fn v2_info(piece_length: i64, tree: Value) -> Vec<u8> {
    bencode::encode(&d(vec![
        ("file tree", tree),
        ("meta version", Value::Int(2)),
        ("name", b(b"t")),
        ("piece length", Value::Int(piece_length)),
    ]))
}

fn err_of(info: &[u8]) -> String {
    parse_info(info).expect_err("hostile info must be rejected").to_string()
}

// ------------------------------------------------------------- fixtures

#[test]
fn pure_v2_fixture() {
    let (mi, info) = parse_torrent(&load("bittorrent-v2-test.torrent")).unwrap();
    assert!(info.has_v2() && !info.has_v1());
    assert_eq!(
        info_hash_v2(&mi.info_bytes).to_hex(),
        "caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e"
    );
    // Truncated v2 for DHT/tracker/handshake.
    assert_eq!(
        info_hash_v2(&mi.info_bytes).to_short().to_hex(),
        "caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa"
    );
    assert_eq!(info.piece_length, 4 << 20);
    assert_eq!(info.files.len(), 11);
    for f in &info.files {
        assert!(f.pieces_root.is_some(), "{:?}", f.path);
        assert_eq!(f.torrent_offset % info.piece_length, 0, "v2 files are piece aligned");
    }
    // readme.txt (61 bytes) is <= piece length: no piece-layers entry.
    assert_eq!(mi.piece_layers.len(), 10);
    let readme = info.files.iter().find(|f| f.path == ["readme.txt"]).unwrap();
    assert!(!mi.piece_layers.iter().any(|(r, _)| Some(*r) == readme.pieces_root));
    validate_metainfo_piece_layers(&mi, &info).unwrap();
}

#[test]
fn hybrid_fixture_is_consistent() {
    let (mi, info) = parse_torrent(&load("bittorrent-v2-hybrid-test.torrent")).unwrap();
    assert!(info.has_v1() && info.has_v2());
    assert_eq!(info_hash_v1(&mi.info_bytes).to_hex(), "631a31dd0a46257d5078c0dee4e66e26f73e42ac");
    assert_eq!(
        info_hash_v2(&mi.info_bytes).to_hex(),
        "d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb"
    );
    // v2 view drops the 8 v1 pad files.
    assert_eq!(info.files.len(), 9);
    assert!(info.files.iter().all(|f| !f.is_padding()));
    assert_eq!(info.num_pieces() as usize, info.pieces.len() / 20);
    assert_eq!(info.num_pieces(), 1715);
    validate_metainfo_piece_layers(&mi, &info).unwrap();
}

/// Every piece layer in the fixtures reduces to its file's pieces root, and a
/// single flipped bit anywhere in a layer breaks it.
#[test]
fn fixture_piece_layers_reduce_to_pieces_roots() {
    for name in ["bittorrent-v2-test.torrent", "bittorrent-v2-hybrid-test.torrent"] {
        let (mi, info) = parse_torrent(&load(name)).unwrap();
        let pad = hash_for_piece_pad(info.piece_length);
        for (root_hash, layer) in &mi.piece_layers {
            let hashes = compact_layer_to_slice_hashes(layer).unwrap();
            assert_eq!(root_with_pad_hash(&hashes, pad).unwrap(), *root_hash, "{name}");
            let mut bad = hashes.clone();
            bad[hashes.len() / 2][7] ^= 1;
            assert_ne!(root_with_pad_hash(&bad, pad).unwrap(), *root_hash);
        }
    }
}

// ------------------------------------------------------------- merkle

fn naive_pad(piece_length: i64) -> [u8; 32] {
    let n = (piece_length / BLOCK_SIZE as i64) as usize;
    root(&vec![[0u8; 32]; n]).unwrap()
}

#[test]
fn piece_pad_matches_naive_definition() {
    for shift in 14..=24 {
        let pl = 1i64 << shift;
        assert_eq!(hash_for_piece_pad(pl), naive_pad(pl), "piece length {pl}");
    }
    assert_eq!(hash_for_piece_pad(BLOCK_SIZE as i64), [0u8; 32]);
    let empty: [u8; 32] = Sha256::digest([]).into();
    assert_eq!(hash_for_piece_pad(0), empty);
}

/// Regression: this used to allocate 2 GiB and hash 64 Mi zero leaves.
#[test]
fn piece_pad_for_huge_piece_length_is_cheap() {
    let t = std::time::Instant::now();
    let h = hash_for_piece_pad(1i64 << 40);
    assert_eq!(h, merkle::pad_hash_at_height(26));
    let _ = hash_for_piece_pad(i64::MAX);
    assert!(t.elapsed() < std::time::Duration::from_secs(1));
}

fn pseudo_random(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// A file's pieces root over 16 KiB leaves, zero-padded to a power of two,
/// equals the root of its piece layer padded with piece pad hashes.
#[test]
fn pieces_root_two_ways_agree() {
    let piece_length = 4 * BLOCK_SIZE; // 64 KiB, 4 leaves per piece
    for len in [
        1,
        BLOCK_SIZE - 1,
        BLOCK_SIZE,
        BLOCK_SIZE + 1,
        piece_length,
        piece_length + 1,
        3 * piece_length - 100,
        5 * piece_length + 3 * BLOCK_SIZE + 17,
    ] {
        let data = pseudo_random(len);
        let mut whole = Hash::new();
        whole.write(&data);
        let file_root = whole.sum();

        // Leaves -> root by hand.
        let leaves: Vec<[u8; 32]> = data
            .chunks(BLOCK_SIZE)
            .map(|c| Sha256::digest(c).into())
            .collect();
        assert_eq!(root_with_pad_hash(&leaves, [0u8; 32]).unwrap(), file_root, "len {len}");

        if len > piece_length {
            let layer: Vec<[u8; 32]> = data
                .chunks(piece_length)
                .map(|c| {
                    let mut h = Hash::new();
                    h.write(c);
                    h.sum_min_length(piece_length)
                })
                .collect();
            let via_layer =
                root_with_pad_hash(&layer, hash_for_piece_pad(piece_length as i64)).unwrap();
            assert_eq!(via_layer, file_root, "len {len}");
        }
    }
}

/// Build all layers of a tree over `leaves` (padded with zero hashes).
fn layers(leaves: &[[u8; 32]]) -> Vec<Vec<[u8; 32]>> {
    let mut cur = leaves.to_vec();
    cur.resize(leaves.len().next_power_of_two(), [0u8; 32]);
    let mut out = vec![cur.clone()];
    while cur.len() > 1 {
        cur = cur
            .chunks(2)
            .map(|p| {
                let mut buf = [0u8; 64];
                buf[..32].copy_from_slice(&p[0]);
                buf[32..].copy_from_slice(&p[1]);
                Sha256::digest(buf).into()
            })
            .collect();
        out.push(cur.clone());
    }
    out
}

#[test]
fn proof_verification() {
    let leaves: Vec<[u8; 32]> = (0..13u8).map(|i| Sha256::digest([i]).into()).collect();
    let tree = layers(&leaves);
    let top = *tree.last().unwrap().first().unwrap();
    let width = leaves.len() as u64; // 13 real nodes, padded to 16
    for length in [2usize, 4, 8, 16] {
        for index in (0..16).step_by(length) {
            let base = &tree[0][index..index + length];
            // Uncles: sibling of the subtree root at each level up to the root.
            let mut uncles = Vec::new();
            let mut pos = index / length;
            let mut level = length.trailing_zeros() as usize;
            while level + 1 < tree.len() {
                uncles.push(tree[level][pos ^ 1]);
                pos /= 2;
                level += 1;
            }
            assert!(
                verify_hashes_with_proof(&top, index as u32, base, &uncles, width),
                "len {length} idx {index}"
            );
            // Tampered base hash, tampered uncle, wrong position, missing uncle.
            let mut bad = base.to_vec();
            bad[0][0] ^= 1;
            assert!(!verify_hashes_with_proof(&top, index as u32, &bad, &uncles, width));
            if !uncles.is_empty() {
                let mut u = uncles.clone();
                u[0][31] ^= 1;
                assert!(!verify_hashes_with_proof(&top, index as u32, base, &u, width));
                assert!(!verify_hashes_with_proof(
                    &top,
                    index as u32,
                    base,
                    &uncles[1..],
                    width
                ));
                let other = ((index + length) % 16) as u32;
                assert!(!verify_hashes_with_proof(&top, other, base, &uncles, width));
            }
        }
    }
    // Shape errors from a hostile peer: no panic, just false.
    assert!(!verify_hashes_with_proof(&top, 0, &[], &[], width));
    assert!(!verify_hashes_with_proof(&top, 1, &tree[0][0..2], &[], width));
    assert!(!verify_hashes_with_proof(&top, 0, &tree[0][0..3], &[], width));
    assert!(!verify_hashes_with_proof(&top, u32::MAX - 1, &tree[0][0..2], &[], width));
    assert!(!verify_hashes_with_proof(&top, 0, &tree[0][0..2], &[[0; 32]; 70], width));
}

// ------------------------------------------------------------- hostile v2 info dicts

const R: [u8; 32] = [7u8; 32];

#[test]
fn baseline_synthetic_v2_info_parses() {
    let tree = d(vec![
        ("dir", d(vec![("a", file_node(100, Some(R))), ("empty", file_node(0, None))])),
        ("z", file_node(20_000, Some(R))),
    ]);
    let info = parse_info(&v2_info(16384, tree)).unwrap();
    let paths: Vec<_> = info.files.iter().map(|f| f.path.join("/")).collect();
    assert_eq!(paths, ["dir/a", "dir/empty", "z"]);
    assert_eq!(info.files[1].torrent_offset, 16384);
    assert_eq!(info.files[1].pieces_root, None);
    assert_eq!(info.files[2].torrent_offset, 16384);
}

#[test]
fn v2_piece_length_must_be_power_of_two_at_least_16k() {
    let tree = || d(vec![("a", file_node(100, Some(R)))]);
    for pl in [0i64, -16384, 1, 8192, 16383, 16385, 3 * 16384, 100_000] {
        assert!(err_of(&v2_info(pl, tree())).contains("piece length"), "pl {pl}");
    }
    for pl in [16384i64, 32768, 1 << 22] {
        parse_info(&v2_info(pl, tree())).unwrap();
    }
}

#[test]
fn meta_version_2_requires_file_tree() {
    let info = bencode::encode(&d(vec![
        ("length", Value::Int(5)),
        ("meta version", Value::Int(2)),
        ("name", b(b"t")),
        ("piece length", Value::Int(16384)),
    ]));
    assert!(err_of(&info).contains("file tree"));
}

#[test]
fn file_tree_path_components_are_validated() {
    for bad in [&b".."[..], b".", b"a/b", b"a\0b"] {
        let tree = Value::Dict(BTreeMap::from([(bad.to_vec(), file_node(1, Some(R)))]));
        assert!(err_of(&v2_info(16384, tree)).contains("path component"), "{bad:?}");
        // Also when nested.
        let tree = d(vec![(
            "dir",
            Value::Dict(BTreeMap::from([(bad.to_vec(), file_node(1, Some(R)))])),
        )]);
        assert!(parse_info(&v2_info(16384, tree)).is_err());
    }
}

#[test]
fn file_tree_shape_is_validated() {
    let cases: Vec<(&str, Value)> = vec![
        ("not a dict", d(vec![("a", Value::Int(1))])),
        ("empty dir", d(vec![("a", d(vec![]))])),
        ("empty tree", d(vec![])),
        ("props not dict", d(vec![("a", d(vec![("", Value::Int(1))]))])),
        ("no length", d(vec![("a", d(vec![("", d(vec![("pieces root", b(&R))]))]))])),
        (
            "string length",
            d(vec![("a", d(vec![("", d(vec![("length", b(b"5")), ("pieces root", b(&R))]))]))]),
        ),
        ("negative length", d(vec![("a", file_node(-1, Some(R)))])),
        ("missing pieces root", d(vec![("a", file_node(5, None))])),
        (
            "short pieces root",
            d(vec![("a", d(vec![("", d(vec![("length", Value::Int(5)), ("pieces root", b(&[1; 20]))]))]))]),
        ),
        (
            "pad file in v2 tree",
            d(vec![(
                "a",
                d(vec![(
                    "",
                    d(vec![("attr", b(b"p")), ("length", Value::Int(5)), ("pieces root", b(&R))]),
                )]),
            )]),
        ),
        ("bare root file", file_node(5, Some(R))),
    ];
    for (what, tree) in cases {
        assert!(parse_info(&v2_info(16384, tree)).is_err(), "{what} accepted");
    }
}

#[test]
fn empty_file_pieces_root_is_ignored() {
    let tree = d(vec![("a", file_node(0, Some(R)))]);
    let info = parse_info(&v2_info(16384, tree)).unwrap();
    assert_eq!(info.files[0].pieces_root, None);
}

#[test]
fn offsets_overflow_is_rejected_not_wrapped() {
    let tree = d(vec![
        ("a", file_node(i64::MAX - 10, Some(R))),
        ("b", file_node(i64::MAX - 10, Some(R))),
    ]);
    assert!(parse_info(&v2_info(16384, tree)).is_err());
    // Fits i64 but not u32 piece/block indexes.
    let tree = d(vec![("a", file_node(1i64 << 50, Some(R)))]);
    assert!(err_of(&v2_info(16384, tree)).contains("too large"));
    // v1 multi-file offsets too.
    let v1 = bencode::encode(&d(vec![
        (
            "files",
            Value::List(vec![
                d(vec![("length", Value::Int(i64::MAX)), ("path", Value::List(vec![b(b"a")]))]),
                d(vec![("length", Value::Int(i64::MAX)), ("path", Value::List(vec![b(b"b")]))]),
            ]),
        ),
        ("name", b(b"t")),
        ("piece length", Value::Int(16384)),
        ("pieces", b(b"")),
    ]));
    assert!(parse_info(&v1).is_err());
    let v1 = bencode::encode(&d(vec![
        (
            "files",
            Value::List(vec![d(vec![("length", Value::Int(-5)), ("path", Value::List(vec![b(b"a")]))])]),
        ),
        ("name", b(b"t")),
        ("piece length", Value::Int(16384)),
        ("pieces", b(b"")),
    ]));
    assert!(parse_info(&v1).is_err());
}

/// Regression: a .torrent whose file tree nests a million levels used to abort
/// the process with a stack overflow.
#[test]
fn deep_file_tree_is_an_error() {
    let n = 100_000;
    let mut tree = Vec::new();
    for _ in 0..n {
        tree.extend_from_slice(b"d1:a");
    }
    tree.extend_from_slice(b"de");
    tree.extend(std::iter::repeat_n(b'e', n));
    let mut info = b"d9:file tree".to_vec();
    info.extend_from_slice(&tree);
    info.extend_from_slice(b"12:meta versioni2e4:name1:t12:piece lengthi16384ee");
    assert!(parse_info(&info).is_err());
    let mut torrent = b"d4:info".to_vec();
    torrent.extend_from_slice(&info);
    torrent.push(b'e');
    assert!(parse_metainfo(&torrent).is_err());
}

#[test]
fn deep_but_reasonable_paths_are_accepted() {
    let mut node = file_node(1, Some(R));
    for i in 0..50 {
        node = Value::Dict(BTreeMap::from([(format!("d{i}").into_bytes(), node)]));
    }
    let info = parse_info(&v2_info(16384, node)).unwrap();
    assert_eq!(info.files[0].path.len(), 50);
}

// ------------------------------------------------------------- hybrid

fn v1_entry(path: &str, len: i64, pad: bool) -> Value {
    let mut e = vec![
        ("length", Value::Int(len)),
        ("path", Value::List(path.split('/').map(|p| b(p.as_bytes())).collect())),
    ];
    if pad {
        e.push(("attr", b(b"p")));
    }
    d(e)
}

fn hybrid(files: Vec<Value>, tree: Value, pieces: usize) -> Vec<u8> {
    bencode::encode(&d(vec![
        ("file tree", tree),
        ("files", Value::List(files)),
        ("meta version", Value::Int(2)),
        ("name", b(b"t")),
        ("piece length", Value::Int(16384)),
        ("pieces", b(&vec![0u8; pieces * 20])),
    ]))
}

fn hybrid_tree() -> Value {
    d(vec![("a", file_node(100, Some(R))), ("b", file_node(20_000, Some(R)))])
}

#[test]
fn hybrid_consistency() {
    let good = || vec![v1_entry("a", 100, false), v1_entry(".pad/16284", 16284, true), v1_entry("b", 20_000, false)];
    parse_info(&hybrid(good(), hybrid_tree(), 3)).unwrap();

    // Missing pad file: v1 offset of b is 100, v2 offset is 16384.
    let no_pad = vec![v1_entry("a", 100, false), v1_entry("b", 20_000, false)];
    assert!(err_of(&hybrid(no_pad, hybrid_tree(), 3)).contains("hybrid"));
    // Renamed file.
    let renamed = vec![v1_entry("a", 100, false), v1_entry(".pad/16284", 16284, true), v1_entry("c", 20_000, false)];
    assert!(err_of(&hybrid(renamed, hybrid_tree(), 3)).contains("hybrid"));
    // Different length.
    let resized = vec![v1_entry("a", 100, false), v1_entry(".pad/16284", 16284, true), v1_entry("b", 20_001, false)];
    assert!(err_of(&hybrid(resized, hybrid_tree(), 3)).contains("hybrid"));
    // Extra v1 file.
    let mut extra = good();
    extra.push(v1_entry("c", 1, false));
    assert!(err_of(&hybrid(extra, hybrid_tree(), 3)).contains("hybrid"));
    // v1 piece count disagrees with the v2 layout.
    assert!(err_of(&hybrid(good(), hybrid_tree(), 2)).contains("piece"));
    assert!(err_of(&hybrid(good(), hybrid_tree(), 4)).contains("piece"));
}

#[test]
fn hybrid_single_file() {
    let info = bencode::encode(&d(vec![
        ("file tree", d(vec![("t", file_node(20_000, Some(R)))])),
        ("length", Value::Int(20_000)),
        ("meta version", Value::Int(2)),
        ("name", b(b"t")),
        ("piece length", Value::Int(16384)),
        ("pieces", b(&[0u8; 40])),
    ]));
    parse_info(&info).unwrap();
    let wrong_len = bencode::encode(&d(vec![
        ("file tree", d(vec![("t", file_node(20_000, Some(R)))])),
        ("length", Value::Int(19_999)),
        ("meta version", Value::Int(2)),
        ("name", b(b"t")),
        ("piece length", Value::Int(16384)),
        ("pieces", b(&[0u8; 40])),
    ]));
    assert!(parse_info(&wrong_len).is_err());
}

// ------------------------------------------------------------- piece layers

#[test]
fn piece_layer_must_have_one_hash_per_piece() {
    let (mut mi, info) = parse_torrent(&load("bittorrent-v2-test.torrent")).unwrap();
    // Appending the pad hash still reduces to the same root when the count
    // stays below the next power of two, so only the length check catches it.
    let pad = hash_for_piece_pad(info.piece_length);
    let idx = mi
        .piece_layers
        .iter()
        .position(|(_, l)| {
            let n = l.len() / 32;
            n + 1 < n.next_power_of_two()
        })
        .expect("fixture has a non-power-of-two layer");
    let (r, layer) = &mi.piece_layers[idx];
    let mut longer = layer.clone();
    longer.extend_from_slice(&pad);
    let hashes = compact_layer_to_slice_hashes(&longer).unwrap();
    assert_eq!(root_with_pad_hash(&hashes, pad).unwrap(), *r);
    mi.piece_layers[idx].1 = longer;
    let err = validate_metainfo_piece_layers(&mi, &info).unwrap_err().to_string();
    assert!(err.contains("expected"), "{err}");

    // Ragged layer.
    let (mut mi, info) = parse_torrent(&load("bittorrent-v2-test.torrent")).unwrap();
    mi.piece_layers[0].1.pop();
    assert!(validate_metainfo_piece_layers(&mi, &info).is_err());

    // Missing layer for a multi-piece file.
    let (mut mi, info) = parse_torrent(&load("bittorrent-v2-test.torrent")).unwrap();
    mi.piece_layers.remove(0);
    assert!(validate_metainfo_piece_layers(&mi, &info).is_err());
}

/// wt-types piece geometry against the fixtures: v2 piece data lengths sum to
/// the file bytes, and for hybrid the v1 view (data + zero pad) tiles the
/// whole payload up to the end of the last file.
#[test]
fn piece_geometry_matches_file_layout() {
    for name in ["bittorrent-v2-test.torrent", "bittorrent-v2-hybrid-test.torrent"] {
        let (_, info) = parse_torrent(&load(name)).unwrap();
        let pl = info.piece_length;
        let n = info.num_pieces();
        let data: i64 = (0..n).map(|i| info.piece_data_length(i)).sum();
        assert_eq!(data, info.total_length(), "{name}");
        for f in &info.files {
            if f.length == 0 {
                continue;
            }
            let last = f.end_piece_index(pl) - 1;
            let tail = f.length - (last - f.begin_piece_index(pl)) as i64 * pl;
            assert_eq!(info.piece_data_length(last), tail, "{name} {:?}", f.path);
        }
        if info.has_v1() {
            let v1: i64 = (0..n).map(|i| info.piece_v1_length(i)).sum();
            assert_eq!(v1, info.payload_end(), "{name}");
            let pads: i64 = (0..n).map(|i| info.v1_zero_pad(i)).sum();
            assert_eq!(data + pads, v1, "{name}");
        } else {
            assert!((0..n).all(|i| info.v1_zero_pad(i) == 0));
        }
    }
}

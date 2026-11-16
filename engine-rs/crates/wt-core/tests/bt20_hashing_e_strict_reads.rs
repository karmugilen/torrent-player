//! Area E: a strict (non-responsive) reader must never see a v2/hybrid block
//! that has not passed either the 16 KiB leaf check or the whole-piece hash.
//! Responsive readers may see dirty unverified mid-piece bytes by design.

use std::sync::Arc;
use std::time::Duration;

use wt_core::hashing::{leaf_layer_of, ExpectedHash, LeafCheck, PieceHasherPool};
use wt_core::peer_conn::PeerTorrent;
use wt_core::reader::ReaderHost;
use wt_core::storage_adapter::MemoryClientStorage;
use wt_core::torrent::TorrentInner;
use wt_core::PiecePriority;
use wt_meta::merkle::{hash_for_piece_pad, root_with_pad_hash, Hash, BLOCK_SIZE};
use wt_types::{FileInfo, Info, InfoHashV2};

const BLOCK: usize = BLOCK_SIZE as usize;
const PIECE: usize = 4 * BLOCK;

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251 + i / BLOCK * 17) as u8).collect()
}

fn v2_torrent() -> (Arc<TorrentInner>, Vec<u8>) {
    let data = payload(2 * PIECE);
    let layer: Vec<[u8; 32]> = data
        .chunks(PIECE)
        .map(|p| {
            let mut h = Hash::new();
            h.write(p);
            h.sum_min_length(PIECE)
        })
        .collect();
    let root = root_with_pad_hash(&layer, hash_for_piece_pad(PIECE as i64)).unwrap();
    let info = Arc::new(Info {
        name: "v2".into(),
        piece_length: PIECE as i64,
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
    let v2 = InfoHashV2([0x77; 32]);
    let t = Arc::new(TorrentInner::new(
        v2.to_short(),
        Some(v2),
        "v2".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info_with_layers(Arc::clone(&info), &[(root, layer.concat())])
        .expect("set info");
    assert!(matches!(
        t.expected_v2.read()[0],
        Some(ExpectedHash::V2PieceLayer(_))
    ));
    (t, data)
}

fn want(t: &TorrentInner, piece: usize) {
    let mut pieces = t.pieces_mut();
    pieces[piece].priority = PiecePriority::Now;
    pieces[piece].base_priority = PiecePriority::Now;
}

fn give_leaves(t: &TorrentInner, data: &[u8], piece: usize) {
    let key = t.leaf_hash_request(piece as u32).expect("leaf request");
    let leaves = leaf_layer_of(&data[piece * PIECE..(piece + 1) * PIECE], key.length as usize);
    assert!(t
        .apply_peer_leaf_hashes(key.pieces_root, key.index, &leaves)
        .unwrap());
}

fn deliver(t: &TorrentInner, piece: u32, block: u32, bytes: &[u8]) -> LeafCheck {
    let begin = block * BLOCK as u32;
    let req = piece * (PIECE / BLOCK) as u32 + block;
    assert!(t.begin_chunk_write(req));
    let leaf = t.check_leaf(piece, begin, bytes);
    match leaf {
        LeafCheck::Bad => t.finish_chunk_write(req, false),
        LeafCheck::Verified => {
            t.write_chunk(piece, begin, bytes).unwrap();
            t.finish_verified_chunk_write(req);
        }
        LeafCheck::Unknown => {
            t.write_chunk(piece, begin, bytes).unwrap();
            t.finish_chunk_write(req, true);
        }
    }
    leaf
}

fn block_of(data: &[u8], piece: usize, block: usize) -> &[u8] {
    let at = piece * PIECE + block * BLOCK;
    &data[at..at + BLOCK]
}

/// Strict readers wait for leaf or piece verification; responsive ones may
/// read dirty unverified blocks (seek latency). A reader is never handed a
/// block that failed its leaf check.
#[test]
fn strict_v2_reader_never_sees_unverified_or_corrupt_blocks() {
    let (t, data) = v2_torrent();
    want(&t, 0);

    // No leaf hashes yet: dirty block is visible to responsive, hidden to strict.
    assert_eq!(
        deliver(&t, 0, 0, block_of(&data, 0, 0)),
        LeafCheck::Unknown
    );
    assert_eq!(t.have(0, BLOCK as i64, false), 0, "strict waits without a leaf");
    assert_eq!(
        t.have(0, BLOCK as i64, true),
        BLOCK as i64,
        "responsive may read dirty"
    );
    let mut buf = vec![0u8; BLOCK];
    // ReaderHost::read_at itself does not gate; have() is the gate the Reader uses.
    assert_eq!(ReaderHost::read_at(t.as_ref(), &mut buf, 0).unwrap(), BLOCK);

    // With leaves: a matching block becomes strict-readable; a corrupt one never is.
    give_leaves(&t, &data, 0);
    // Block 0 was already dirty before leaves arrived: verify_dirty_leaves promotes it.
    assert!(
        t.verified_chunks.contains(0),
        "stored good block is promoted once leaves arrive"
    );
    assert_eq!(t.have(0, BLOCK as i64, false), BLOCK as i64);

    let mut bad = block_of(&data, 0, 1).to_vec();
    bad[0] ^= 0xff;
    assert_eq!(deliver(&t, 0, 1, &bad), LeafCheck::Bad);
    assert!(!t.chunk_dirty(1));
    assert_eq!(t.have(BLOCK as i64, BLOCK as i64, false), 0);
    assert_eq!(t.have(BLOCK as i64, BLOCK as i64, true), 0);

    assert_eq!(
        deliver(&t, 0, 1, block_of(&data, 0, 1)),
        LeafCheck::Verified
    );
    assert_eq!(t.have(0, 2 * BLOCK as i64, false), 2 * BLOCK as i64);

    // Whole-piece verify also unlocks the remaining dirty (un-leaf-checked) path
    // once the piece is complete — already covered in leaf_verify.rs; here we
    // only pin that a gap without a leaf stays closed to strict readers.
    assert_eq!(
        deliver(&t, 0, 3, block_of(&data, 0, 3)),
        LeafCheck::Verified
    );
    assert_eq!(t.have(2 * BLOCK as i64, BLOCK as i64, false), 0);
    assert_eq!(t.have(3 * BLOCK as i64, BLOCK as i64, false), BLOCK as i64);
}

/// After the piece hash succeeds, strict readers see the whole piece even
/// without per-block leaf marks (v1-equivalent path on a completed piece).
#[test]
fn completed_piece_is_strict_readable_without_leaf_marks() {
    let (t, data) = v2_torrent();
    want(&t, 1);
    // Deliver all blocks without leaf hashes (Unknown).
    for b in 0..(PIECE / BLOCK) {
        assert_eq!(
            deliver(&t, 1, b as u32, block_of(&data, 1, b)),
            LeafCheck::Unknown
        );
    }
    assert_eq!(t.have(PIECE as i64, PIECE as i64, false), 0);
    assert!(PeerTorrent::piece_all_dirty(t.as_ref(), 1));
    let (pool, rx) = PieceHasherPool::with_workers(1);
    *t.hasher.lock() = Some(pool);
    PeerTorrent::queue_piece_hash(t.as_ref(), 1);
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("hash");
    assert!(result.correct);
    // Apply the same completion path the hasher callback uses.
    t.pieces_mut()[1].complete = true;
    assert_eq!(t.have(PIECE as i64, PIECE as i64, false), PIECE as i64);
}

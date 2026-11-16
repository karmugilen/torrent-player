//! BEP 52 wiring at the `TorrentInner` level: ut_metadata verification per
//! magnet kind, piece-layer gating of verification, and hash-exchange input
//! from untrusted peers.

use std::sync::Arc;

use bytes::Bytes;
use wt_core::peer_conn::PeerTorrent;
use wt_core::{InfoHash, InfoHashV2, MemoryClientStorage, TorrentInner};

const PURE_V2: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-test.torrent");
const HYBRID: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");
const METADATA_PIECE: usize = 16 * 1024;

fn info_bytes(file: &[u8]) -> Vec<u8> {
    wt_meta::parse_metainfo(file).unwrap().info_bytes
}

fn torrent(v1: InfoHash, v2: Option<InfoHashV2>) -> Arc<TorrentInner> {
    Arc::new(TorrentInner::new(v1, v2, String::new(), Arc::new(MemoryClientStorage::new())))
}

/// Feed `bytes` through ut_metadata; `Ok(true)` once it verified.
fn feed_metadata(t: &TorrentInner, bytes: &[u8]) -> wt_core::Result<bool> {
    let mut done = false;
    for (i, chunk) in bytes.chunks(METADATA_PIECE).enumerate() {
        done = t.on_metadata_data(i as u32, bytes.len() as u32, Bytes::copy_from_slice(chunk))?;
    }
    Ok(done)
}

#[test]
fn metadata_verifies_for_every_magnet_kind() {
    let hy = info_bytes(HYBRID);
    let (hv1, hv2) = (wt_meta::info_hash_v1(&hy), wt_meta::info_hash_v2(&hy));
    let pv = info_bytes(PURE_V2);
    let pv2 = wt_meta::info_hash_v2(&pv);

    let cases: [(&str, InfoHash, Option<InfoHashV2>, &[u8]); 4] = [
        ("btih of a hybrid", hv1, None, &hy),
        ("btmh of a hybrid", hv2.to_short(), Some(hv2), &hy),
        ("btih+btmh hybrid", hv1, Some(hv2), &hy),
        ("pure btmh", pv2.to_short(), Some(pv2), &pv),
    ];
    for (name, v1, v2, bytes) in cases {
        let t = torrent(v1, v2);
        assert!(feed_metadata(&t, bytes).unwrap(), "{name}: metadata must verify");
        assert!(t.info.read().as_ref().is_some_and(|i| i.has_v2()), "{name}");
    }
}

#[test]
fn metadata_for_another_torrent_is_rejected() {
    let pv = info_bytes(PURE_V2);
    let pv2 = wt_meta::info_hash_v2(&pv);
    // A pure v2 magnet must not accept the hybrid's info dict.
    let t = torrent(pv2.to_short(), Some(pv2));
    assert!(!matches!(feed_metadata(&t, &info_bytes(HYBRID)), Ok(true)));
    assert!(t.info.read().is_none());
}

/// Pure v2 from a magnet has no piece layers (no v1 hashes either): nothing
/// can be verified until a peer sends them, and hash exchange knows which
/// files to ask for.
#[test]
fn pure_v2_magnet_waits_for_piece_layers_before_verifying() {
    let pv = info_bytes(PURE_V2);
    let pv2 = wt_meta::info_hash_v2(&pv);
    let t = torrent(pv2.to_short(), Some(pv2));
    assert!(feed_metadata(&t, &pv).unwrap());
    assert!(t.has_v2());
    let missing = t.missing_piece_layer_files();
    assert!(!missing.is_empty(), "multi-piece files need piece layers");
    let piece = missing[0].begin_piece;
    t.queue_piece_hash(piece);
    assert!(
        !t.pieces.read()[piece as usize].checking,
        "piece of a file without layers must not be hashed"
    );
}

/// `Hashes` naming a root that is no file of this torrent is refused and
/// leaves no buffer behind: a peer cannot grow torrent memory with them.
#[test]
fn hashes_for_unknown_roots_leave_no_buffer() {
    let pv = info_bytes(PURE_V2);
    let pv2 = wt_meta::info_hash_v2(&pv);
    let t = torrent(pv2.to_short(), Some(pv2));
    assert!(feed_metadata(&t, &pv).unwrap());
    for n in 0..1000u32 {
        let mut root = [0u8; 32];
        root[..4].copy_from_slice(&n.to_le_bytes());
        assert!(t.apply_peer_piece_hashes(root, 0, &[[1u8; 32]; 2]).is_err());
    }
    assert!(t.pending_piece_layers.read().is_empty());

    // A real file's root with wrong hashes: one bounded buffer, no commit.
    let file = t.missing_piece_layer_files().remove(0);
    let bogus = vec![[7u8; 32]; file.num_pieces as usize];
    assert!(!t.apply_peer_piece_hashes(file.pieces_root, 0, &bogus).unwrap());
    assert!(t.pending_piece_layers.read().len() <= 1);
    assert!(t.missing_piece_layer_files().iter().any(|f| f.pieces_root == file.pieces_root));
}

//! BEP 52 block-level (16 KiB leaf) verification: reads before a piece is whole.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sha1::{Digest, Sha1};
use tokio::sync::{mpsc, Notify};
use wt_core::conn::{ConnHandle, ConnHandler};
use wt_core::hashing::{leaf_layer_of, ExpectedHash, HashRequestKey, LeafCheck, PieceHasherPool};
use wt_core::peer_conn::{PeerConnHandler, PeerTorrent};
use wt_core::reader::{Reader, ReaderHost};
use wt_core::storage_adapter::MemoryClientStorage;
use wt_core::torrent::TorrentInner;
use wt_core::PiecePriority;
use wt_meta::merkle::{hash_for_piece_pad, root_with_pad_hash, Hash};
use wt_types::{FileInfo, Info, InfoHash, InfoHashV2, PeerSource, Transport};
use wt_wire::handshake::default_peer_extension_bytes;
use wt_wire::message::Message;

const BLOCK: usize = 16 * 1024;
/// Four blocks per piece.
const PIECE: usize = 4 * BLOCK;
const V2_HASH: [u8; 32] = [0x77; 32];

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251 + i / BLOCK * 17) as u8).collect()
}

fn file(len: usize, pieces_root: Option<[u8; 32]>) -> Vec<FileInfo> {
    vec![FileInfo {
        path: vec!["f.bin".into()],
        length: len as i64,
        torrent_offset: 0,
        attr: String::new(),
        pieces_root,
    }]
}

/// A v2 torrent of two full pieces with its piece layer, and the payload.
fn v2_torrent() -> (Arc<TorrentInner>, Arc<Info>, Vec<u8>) {
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
        files: file(data.len(), Some(root)),
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 2,
        file_tree_raw: None,
    });
    let v2 = InfoHashV2(V2_HASH);
    let t = Arc::new(TorrentInner::new(
        v2.to_short(),
        Some(v2),
        "v2".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info_with_layers(Arc::clone(&info), &[(root, layer.concat())]).expect("set info");
    assert!(matches!(t.expected_v2.read()[0], Some(ExpectedHash::V2PieceLayer(_))));
    (t, info, data)
}

fn want(t: &TorrentInner, piece: usize) {
    let mut pieces = t.pieces_mut();
    pieces[piece].priority = PiecePriority::Now;
    pieces[piece].base_priority = PiecePriority::Now;
}

/// Fetch and apply piece `piece`'s leaf hashes as a peer exchange would.
fn give_leaves(t: &TorrentInner, data: &[u8], piece: usize) -> HashRequestKey {
    let key = t.leaf_hash_request(piece as u32).expect("leaf request for a wanted v2 piece");
    let leaves = leaf_layer_of(&data[piece * PIECE..(piece + 1) * PIECE], key.length as usize);
    assert!(t.apply_peer_leaf_hashes(key.pieces_root, key.index, &leaves).unwrap());
    key
}

/// Deliver one block the way the peer handler does.
fn deliver(t: &TorrentInner, piece: u32, block: u32, bytes: &[u8]) -> LeafCheck {
    let begin = block * BLOCK as u32;
    let req = piece * (PIECE / BLOCK) as u32 + block;
    assert!(t.begin_chunk_write(req), "block must be free to write");
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

#[test]
fn verified_leaves_are_readable_before_the_piece_completes() {
    let (t, _info, data) = v2_torrent();
    want(&t, 0);
    let key = give_leaves(&t, &data, 0);
    assert_eq!((key.base_layer, key.index, key.length, key.proof_layers), (0, 0, 4, 0));
    // Already held: nothing more to ask.
    assert!(t.leaf_hash_request(0).is_none());

    // Blocks 0, 1 and 3 arrive; block 2 does not.
    for b in [0usize, 1, 3] {
        assert_eq!(deliver(&t, 0, b as u32, block_of(&data, 0, b)), LeafCheck::Verified);
    }
    assert!(!t.pieces.read()[0].complete);
    assert!(!PeerTorrent::piece_all_dirty(t.as_ref(), 0));

    // A strict (non-responsive) read sees exactly the verified prefix.
    assert_eq!(t.have(0, PIECE as i64, false), 2 * BLOCK as i64);
    assert_eq!(t.have(100, 50, false), 50);
    assert_eq!(t.have(2 * BLOCK as i64, 1, false), 0, "the gap is not readable");
    assert_eq!(t.have(3 * BLOCK as i64, PIECE as i64, false), BLOCK as i64, "piece 1 has nothing");

    let mut buf = vec![0u8; 2 * BLOCK];
    assert_eq!(ReaderHost::read_at(t.as_ref(), &mut buf, 0).unwrap(), 2 * BLOCK);
    assert_eq!(buf, &data[..2 * BLOCK]);

    // Through the reader: the prefix reads, the unverified gap still waits.
    let mut reader = Reader::new(Arc::clone(&t), 0, data.len() as i64);
    let mut got = Vec::new();
    let mut chunk = vec![0u8; PIECE];
    while got.len() < 2 * BLOCK {
        let n = reader.read_timeout(&mut chunk, Some(Duration::from_secs(5))).expect("verified read");
        assert!(n > 0);
        got.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(got, &data[..2 * BLOCK]);
    let err = reader.read_timeout(&mut chunk, Some(Duration::from_millis(60))).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::TimedOut);

    // The gap fills: the read continues, and the piece hash runs as before.
    assert_eq!(deliver(&t, 0, 2, block_of(&data, 0, 2)), LeafCheck::Verified);
    assert_eq!(t.have(0, PIECE as i64, false), PIECE as i64);
    let n = reader.read_timeout(&mut chunk, Some(Duration::from_secs(5))).expect("gap filled");
    assert_eq!(&chunk[..n], &data[2 * BLOCK..2 * BLOCK + n]);

    assert!(PeerTorrent::piece_all_dirty(t.as_ref(), 0));
    let (pool, rx) = PieceHasherPool::with_workers(1);
    *t.hasher.lock() = Some(pool);
    PeerTorrent::queue_piece_hash(t.as_ref(), 0);
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("piece hash result");
    assert!(result.correct, "the whole piece still passes its piece-layer hash");
}

#[test]
fn corrupt_block_is_rejected_and_never_readable() {
    let (t, _info, data) = v2_torrent();
    want(&t, 0);
    give_leaves(&t, &data, 0);

    let mut bad = block_of(&data, 0, 0).to_vec();
    bad[9] ^= 0x40;
    assert_eq!(deliver(&t, 0, 0, &bad), LeafCheck::Bad);
    assert!(!t.chunk_dirty(0), "a bad block is not written");
    assert_eq!(t.have(0, BLOCK as i64, false), 0);
    assert_eq!(t.have(0, BLOCK as i64, true), 0, "not even to a responsive reader");

    // The block is free again and the good copy lands.
    assert_eq!(deliver(&t, 0, 0, block_of(&data, 0, 0)), LeafCheck::Verified);
    assert_eq!(t.have(0, BLOCK as i64, false), BLOCK as i64);
}

#[test]
fn forged_leaf_hashes_are_not_trusted() {
    let (t, _info, data) = v2_torrent();
    want(&t, 0);
    let key = t.leaf_hash_request(0).unwrap();
    let mut bad = block_of(&data, 0, 1).to_vec();
    bad[0] ^= 1;
    // A peer lying about leaf 1 to pass off its own bad block.
    let mut leaves = leaf_layer_of(&data[..PIECE], 4);
    leaves[1] = wt_core::hashing::leaf_hash(&bad);
    assert!(!t.apply_peer_leaf_hashes(key.pieces_root, key.index, &leaves).unwrap());
    assert!(t.leaf_layers.read().is_empty());

    // Without trusted leaves the block is only piece-verifiable, as before.
    assert_eq!(deliver(&t, 0, 1, &bad), LeafCheck::Unknown);
    assert_eq!(t.have(BLOCK as i64, 1, false), 0);
    // Another peer may be asked straight away.
    assert!(t.leaf_hash_request(0).is_some());
    // A span that is not one piece is refused outright.
    assert!(t.apply_peer_leaf_hashes(key.pieces_root, 0, &leaves[..2]).is_err());
}

#[test]
fn blocks_written_before_the_leaves_arrive_are_checked_when_they_do() {
    let (t, _info, data) = v2_torrent();
    want(&t, 1);
    let mut bad = block_of(&data, 1, 2).to_vec();
    bad[BLOCK - 1] ^= 0xff;
    assert_eq!(deliver(&t, 1, 0, block_of(&data, 1, 0)), LeafCheck::Unknown);
    assert_eq!(deliver(&t, 1, 2, &bad), LeafCheck::Unknown);
    assert_eq!(t.have(PIECE as i64, 1, false), 0, "unverified: strict readers wait");

    give_leaves(&t, &data, 1);
    assert_eq!(t.have(PIECE as i64, PIECE as i64, false), BLOCK as i64);
    let bad_req = 4 + 2;
    assert!(!t.chunk_dirty(bad_req), "the stored bad block is dropped for refetch");
    assert_eq!(deliver(&t, 1, 2, block_of(&data, 1, 2)), LeafCheck::Verified);
}

#[test]
fn leaf_hashes_are_only_fetched_where_a_reader_wants_them() {
    let (t, _info, data) = v2_torrent();
    assert!(t.leaf_hash_request(0).is_none(), "unwanted piece");
    t.pieces_mut()[0].priority = PiecePriority::Normal;
    assert!(t.leaf_hash_request(0).is_none(), "background download");
    want(&t, 0);
    assert!(t.leaf_hash_request(0).is_some());
    assert!(t.leaf_hash_request(0).is_none(), "one peer is asked at a time");

    // We can answer the same request for a piece we hold leaves for.
    let leaves = leaf_layer_of(&data[..PIECE], 4);
    let root = t.info.read().as_ref().unwrap().files[0].pieces_root.unwrap();
    assert!(t.serve_leaf_hashes(root, 0, 4, 0).is_err(), "nothing held yet");
    assert!(t.apply_peer_leaf_hashes(root, 0, &leaves).unwrap());
    assert_eq!(t.serve_leaf_hashes(root, 0, 4, 0).unwrap(), leaves);
    assert!(t.serve_leaf_hashes(root, 0, 8, 0).is_err());
    // proof_layers=1 is absorbed by the omitted leaf fold (log2(4)-1 = 1);
    // a full proof to the pieces root needs one more uncle (the other piece).
    assert_eq!(t.serve_leaf_hashes(root, 0, 4, 1).unwrap(), leaves);
    let with_proof = t.serve_leaf_hashes(root, 0, 4, 2).unwrap();
    assert_eq!(with_proof.len(), 5);
    assert_eq!(&with_proof[..4], &leaves[..]);
}

#[test]
fn v1_incomplete_piece_is_not_readable_until_its_full_hash() {
    let data = payload(2 * PIECE);
    let mut pieces = Vec::new();
    for p in data.chunks(PIECE) {
        pieces.extend_from_slice(&Sha1::digest(p));
    }
    let info = Arc::new(Info {
        name: "v1".into(),
        piece_length: PIECE as i64,
        pieces,
        files: file(data.len(), None),
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 1,
        file_tree_raw: None,
    });
    let t = Arc::new(TorrentInner::new(
        InfoHash([0x33; 20]),
        None,
        "v1".into(),
        Arc::new(MemoryClientStorage::new()),
    ));
    t.set_info(Arc::clone(&info)).unwrap();
    want(&t, 0);

    assert!(t.leaf_hash_request(0).is_none(), "v1 has no leaves to ask for");
    assert!(t.serve_leaf_hashes([0; 32], 0, 4, 0).is_err());
    for b in 0..4usize {
        assert_eq!(deliver(&t, 0, b as u32, block_of(&data, 0, b)), LeafCheck::Unknown);
        assert_eq!(t.have(0, PIECE as i64, false), 0, "v1 block {b} must wait for the piece hash");
    }
    // Responsive readers keep their existing unverified early view.
    assert_eq!(t.have(0, PIECE as i64, true), PIECE as i64);

    let mut reader = Reader::new(Arc::clone(&t), 0, data.len() as i64);
    let mut buf = vec![0u8; BLOCK];
    let err = reader.read_timeout(&mut buf, Some(Duration::from_millis(60))).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::TimedOut);

    // Whole-piece verification is what releases it.
    let (pool, rx) = PieceHasherPool::with_workers(1);
    *t.hasher.lock() = Some(pool);
    PeerTorrent::queue_piece_hash(t.as_ref(), 0);
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("hash result");
    assert!(result.correct);
}

fn v2_handle(addr: SocketAddr) -> (ConnHandle, mpsc::UnboundedReceiver<Message>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = ConnHandle::new(
        addr,
        true,
        Transport::Tcp,
        false,
        InfoHashV2(V2_HASH).to_short(),
        Some([9u8; 20]),
        PeerSource::Direct,
        default_peer_extension_bytes().0,
        tx,
        Arc::new(Notify::new()),
        Arc::new(AtomicBool::new(false)),
        Arc::new(wt_core::stats::AtomicConnStats::default()),
    );
    (handle, rx)
}

fn drain(rx: &mut mpsc::UnboundedReceiver<Message>) -> Vec<Message> {
    let mut out = Vec::new();
    while let Ok(m) = rx.try_recv() {
        out.push(m);
    }
    out
}

/// The wire flow: the leecher asks a v2 peer for a piece's leaves ahead of its
/// blocks, a seeder answers from its stored piece, and each block is then
/// readable on arrival; a corrupt block ends the connection unwritten.
#[test]
fn peer_handler_fetches_leaves_then_verifies_each_block() {
    // Seeder: holds piece 0 complete.
    let (seeder, seed_info, data) = v2_torrent();
    {
        let storage = seeder.storage.read().clone().unwrap();
        let ps = storage.piece(seed_info.piece(0), None);
        ps.write_at(&data[..PIECE], 0).unwrap();
        ps.mark_complete().unwrap();
        seeder.pieces_mut()[0].complete = true;
    }
    let (seed_handle, mut seed_rx) = v2_handle("127.0.0.1:5201".parse().unwrap());
    let mut seed_peer = PeerConnHandler::new(Arc::clone(&seeder), &seed_handle);

    // Leecher: a reader waits at the start of the file.
    let (leecher, _info, _) = v2_torrent();
    want(&leecher, 0);
    let (handle, mut rx) = v2_handle("127.0.0.1:5202".parse().unwrap());
    let mut peer = PeerConnHandler::new(Arc::clone(&leecher), &handle);
    peer.on_message(&handle, Message::HaveAll).unwrap();
    peer.on_message(&handle, Message::Unchoke).unwrap();
    peer.update_requests(&handle);

    let sent = drain(&mut rx);
    let hash_at = sent
        .iter()
        .position(|m| matches!(m, Message::HashRequest { base_layer: 0, index: 0, length: 4, .. }))
        .expect("leaf HashRequest for piece 0");
    let block_at = sent
        .iter()
        .position(|m| matches!(m, Message::Request { index: 0, .. }))
        .expect("block request for piece 0");
    assert!(hash_at < block_at, "leaves are asked for ahead of the blocks");
    let requested: Vec<u32> = sent
        .iter()
        .filter_map(|m| match m {
            Message::Request { index: 0, begin, .. } => Some(*begin),
            _ => None,
        })
        .collect();
    assert!(requested.contains(&0));

    // The seeder answers the leaf request from its stored piece.
    seed_peer.on_message(&seed_handle, sent[hash_at].clone()).unwrap();
    let hashes = drain(&mut seed_rx)
        .into_iter()
        .find(|m| matches!(m, Message::Hashes { base_layer: 0, .. }))
        .expect("seeder serves leaf hashes");
    peer.on_message(&handle, hashes).unwrap();
    assert!(leecher.leaf_layers.read().contains_key(&0));

    // First block: readable by a strict reader the moment it lands.
    peer.on_message(
        &handle,
        Message::Piece { index: 0, begin: 0, piece: Bytes::copy_from_slice(&data[..BLOCK]) },
    )
    .unwrap();
    assert_eq!(leecher.have(0, PIECE as i64, false), BLOCK as i64);
    assert!(!leecher.pieces.read()[0].complete);

    // A corrupt block: rejected, unwritten, and the peer is dropped.
    let begin = *requested.iter().find(|&&b| b != 0).expect("a second block was requested");
    let mut bad = data[begin as usize..begin as usize + BLOCK].to_vec();
    bad[3] ^= 0x10;
    let err = peer.on_message(&handle, Message::Piece { index: 0, begin, piece: Bytes::from(bad) });
    assert!(err.is_err(), "a block failing its merkle leaf ends the connection");
    assert!(!leecher.chunk_dirty(begin / BLOCK as u32));
    assert_eq!(leecher.have(i64::from(begin), 1, true), 0);
}

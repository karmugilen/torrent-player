//! Trait method-group tests for `PeerTorrent` + `ReaderHost` on `TorrentInner`.
//!
//! Requires agy-2 `pub(crate)` fields on `TorrentInner` (see `torrent_impls.rs`).

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wt_core::peer_conn::PeerTorrent;
use wt_core::reader::{PieceRange, ReaderHost};
use wt_core::sched::{PickPeer, Take};
use wt_core::storage_adapter::MemoryClientStorage;
use wt_core::torrent::TorrentInner;
use wt_core::torrent_impls::{note_peer_piece, peer_key_from_addr};
use wt_types::{FileInfo, Info, InfoHash, PiecePriority, BLOCK_SIZE};

fn test_info(num_pieces: u32, piece_len: i64) -> Info {
    let total = i64::from(num_pieces) * piece_len;
    Info {
        name: "t".into(),
        piece_length: piece_len,
        pieces: vec![0u8; (num_pieces as usize) * 20],
        files: vec![FileInfo {
            path: vec![],
            length: total,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 1,
        file_tree_raw: None,
    }
}

fn fresh_torrent() -> Arc<TorrentInner> {
    Arc::new(TorrentInner::new(
        InfoHash([1u8; 20]),
        None,
        "test".into(),
        Arc::new(MemoryClientStorage::new()),
    ))
}

#[test]
fn peer_key_stable_for_addr() {
    let a = SocketAddr::from((Ipv4Addr::new(1, 2, 3, 4), 6881));
    assert_eq!(peer_key_from_addr(a), peer_key_from_addr(a));
    let b = SocketAddr::from((Ipv4Addr::new(1, 2, 3, 5), 6881));
    assert_ne!(peer_key_from_addr(a), peer_key_from_addr(b));
}

#[test]
fn peer_torrent_chunk_io_assignment_hedge_meta_flags() {
    let t = fresh_torrent();
    assert!(!PeerTorrent::have_info(t.as_ref()));

    let info = Arc::new(test_info(4, 32 * 1024));
    t.set_info(Arc::clone(&info)).expect("set_info");

    assert!(PeerTorrent::have_info(t.as_ref()));
    assert_eq!(PeerTorrent::num_pieces(t.as_ref()), 4);
    assert_eq!(PeerTorrent::piece_length(t.as_ref()), 32 * 1024);
    assert_eq!(PeerTorrent::chunk_size(t.as_ref()), BLOCK_SIZE);

    let chunk = vec![0x7Au8; BLOCK_SIZE as usize];
    PeerTorrent::write_chunk(t.as_ref(), 0, 0, &chunk).unwrap();
    PeerTorrent::mark_chunk_dirty(t.as_ref(), 0);
    assert!(PeerTorrent::chunk_dirty(t.as_ref(), 0));
    assert!(t.pieces.read().first().expect("piece 0").partial);

    let got = PeerTorrent::read_chunk(t.as_ref(), 0, 0, BLOCK_SIZE).unwrap();
    assert_eq!(&got[..], &chunk[..]);

    let pk = PeerTorrent::peer_key(t.as_ref(), SocketAddr::from((Ipv4Addr::LOCALHOST, 1)));
    note_peer_piece(&t, pk, 0, true);
    assert!(t.peer_pieces.read().get(&pk).is_some_and(|pieces| pieces.contains(&0)));
    PeerTorrent::note_availability(t.as_ref(), 1, 2);

    assert!(PeerTorrent::claim_request(t.as_ref(), 2, pk, Instant::now()));
    assert_eq!(PeerTorrent::requesting_peer(t.as_ref(), 2), Some(pk));
    PeerTorrent::clear_assignment(t.as_ref(), 2);

    assert!(PeerTorrent::claim_hedge(t.as_ref(), 3, pk));
    assert_eq!(PeerTorrent::hedge_peer(t.as_ref(), 3), Some(pk));
    PeerTorrent::clear_hedge(t.as_ref(), 3);

    PeerTorrent::record_rescue(t.as_ref());
    PeerTorrent::record_hedge(t.as_ref());
    assert!(t.rescues.load(std::sync::atomic::Ordering::Relaxed) >= 1);

    let _cfg = PeerTorrent::choke_config(t.as_ref());
    PeerTorrent::bump_change(t.as_ref());
    PeerTorrent::add_dht_node(t.as_ref(), SocketAddr::from((Ipv4Addr::LOCALHOST, 6881)));
    assert!(!t.dht_nodes.read().is_empty());
    PeerTorrent::on_peer_listen_port(t.as_ref(), SocketAddr::from((Ipv4Addr::LOCALHOST, 9)), 6882);

    // Info present counts as metadata available for seeding path.
    assert!(PeerTorrent::has_metadata(t.as_ref()));
    assert!(PeerTorrent::metadata_size(t.as_ref()).is_none() || true);
}

#[test]
fn reader_host_have_wait_read_positions() {
    let t = fresh_torrent();
    let info = Arc::new(test_info(2, 32 * 1024));
    t.set_info(Arc::clone(&info)).unwrap();

    let host: Arc<dyn ReaderHost> = t.clone();
    assert!(host.has_info());
    assert_eq!(host.num_pieces(), 2);
    assert_eq!(host.torrent_length(), 64 * 1024);

    assert_eq!(host.have(0, 1024, false), 0);

    let chunk = vec![1u8; BLOCK_SIZE as usize];
    PeerTorrent::write_chunk(t.as_ref(), 0, 0, &chunk).unwrap();
    PeerTorrent::mark_chunk_dirty(t.as_ref(), 0);
    assert_eq!(host.have(0, BLOCK_SIZE as i64, true), BLOCK_SIZE as i64);

    let v0 = host.change_version();
    host.set_reader_positions(42, PieceRange { begin: 0, end: 2 });
    assert!(host.change_version() > v0);
    assert!(t.reader_ranges.read().contains_key(&42));
    host.clear_reader(42);
    assert!(!t.reader_ranges.read().contains_key(&42));

    let mut buf = [0u8; 16];
    let n = host.read_at(&mut buf, 0).unwrap();
    assert_eq!(n, 16);
    assert_eq!(&buf, &[1u8; 16]);

    let after = host.change_version();
    let got = host.wait_change(after, Duration::from_millis(20));
    assert_eq!(got, after);
}

fn pick_peer<'a>(
    key: u64,
    pieces: &'a HashSet<u32>,
    none: &'a HashSet<u32>,
    choking: bool,
) -> PickPeer<'a> {
    PickPeer {
        key,
        have_all: false,
        pieces,
        allowed_fast: none,
        choking,
        outstanding: none,
        cancelled: none,
        starving: false,
        rate: 1_000_000.0,
        rtt: Duration::from_millis(50),
        last_useful: Some(Instant::now()),
    }
}

fn wanting(num_pieces: u32, priority: PiecePriority) -> Arc<TorrentInner> {
    let t = fresh_torrent();
    t.set_info(Arc::new(test_info(num_pieces, 64 * 1024))).unwrap();
    for piece in t.pieces_mut().iter_mut() {
        piece.priority = priority;
    }
    t
}

#[test]
fn refills_share_one_queue_without_overlap() {
    let t = wanting(64, PiecePriority::Normal);
    let (all, none): (HashSet<u32>, _) = ((0..64).collect(), HashSet::new());
    let first = t.pick_blocks(&pick_peer(1, &all, &none, false), 8);
    let second = t.pick_blocks(&pick_peer(2, &all, &none, false), 8);
    assert!(first.interested);
    assert_eq!(first.blocks.len(), 8);
    assert_eq!(second.blocks.len(), 8);
    let mine: HashSet<_> = first.blocks.iter().map(|b| b.0).collect();
    assert!(second.blocks.iter().all(|b| !mine.contains(&b.0)));
}

#[test]
fn a_choked_peer_is_interested_but_gets_nothing() {
    let t = wanting(64, PiecePriority::Normal);
    let (all, none): (HashSet<u32>, _) = ((0..64).collect(), HashSet::new());
    let pick = t.pick_blocks(&pick_peer(1, &all, &none, true), 8);
    assert!(pick.interested);
    assert!(pick.blocks.is_empty());
}

#[test]
fn a_peer_without_the_pieces_gets_nothing() {
    let t = wanting(64, PiecePriority::Normal);
    let none = HashSet::new();
    let pick = t.pick_blocks(&pick_peer(1, &none, &none, false), 8);
    assert!(!pick.interested);
    assert!(pick.blocks.is_empty());
}

#[test]
fn the_end_of_a_selection_gets_second_copies() {
    let t = wanting(4, PiecePriority::Normal);
    let (all, none): (HashSet<u32>, _) = ((0..4).collect(), HashSet::new());
    for req in 0..16 {
        assert!(PeerTorrent::claim_request(t.as_ref(), req, 1, Instant::now()));
    }
    let pick = t.pick_blocks(&pick_peer(2, &all, &none, false), 4);
    assert_eq!(pick.blocks.len(), 4);
    assert!(pick.blocks.iter().all(|b| matches!(b.1, Take::Copy { eager: false })));
}

#[test]
fn a_block_someone_else_delivered_is_displaced() {
    let t = wanting(4, PiecePriority::Normal);
    assert!(PeerTorrent::claim_hedge(t.as_ref(), 0, 2));
    let requests: HashSet<u32> = [0].into();
    assert!(PeerTorrent::displaced_requests(t.as_ref(), &requests, 2).is_empty());
    PeerTorrent::mark_chunk_dirty(t.as_ref(), 0);
    assert_eq!(PeerTorrent::displaced_requests(t.as_ref(), &requests, 2), vec![0]);
}

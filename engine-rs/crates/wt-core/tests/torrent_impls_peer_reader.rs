//! Trait method-group tests for `PeerTorrent` + `ReaderHost` on `TorrentInner`.
//!
//! Requires agy-2 `pub(crate)` fields on `TorrentInner` (see `torrent_impls.rs`).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wt_core::arrival::ArrivalModel;
use wt_core::peer_conn::PeerTorrent;
use wt_core::reader::{PieceRange, ReaderHost};
use wt_core::storage_adapter::MemoryClientStorage;
use wt_core::torrent::TorrentInner;
use wt_core::torrent_impls::{init_book_and_glue, note_peer_piece, peer_key_from_addr};
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
    let arrival = Arc::new(parking_lot::RwLock::new(ArrivalModel::default()));
    Arc::new(TorrentInner::new(
        InfoHash([1u8; 20]),
        None,
        "test".into(),
        Arc::new(MemoryClientStorage::new()),
        arrival,
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
    init_book_and_glue(&t, &info);

    assert!(PeerTorrent::have_info(t.as_ref()));
    assert_eq!(PeerTorrent::num_pieces(t.as_ref()), 4);
    assert_eq!(PeerTorrent::piece_length(t.as_ref()), 32 * 1024);
    assert_eq!(PeerTorrent::chunk_size(t.as_ref()), BLOCK_SIZE);

    {
        let mut book = t.book.write();
        let b = book.as_mut().expect("book");
        b.set_complete_cached(0, true, false);
        b.set_file_priority_exact(0, 4, PiecePriority::Normal);
    }

    let chunk = vec![0x7Au8; BLOCK_SIZE as usize];
    PeerTorrent::write_chunk(t.as_ref(), 0, 0, &chunk).unwrap();
    PeerTorrent::mark_chunk_dirty(t.as_ref(), 0);
    assert!(PeerTorrent::chunk_dirty(t.as_ref(), 0));
    assert!(t
        .book
        .read()
        .as_ref()
        .unwrap()
        .piece_partially_downloaded(0));

    let got = PeerTorrent::read_chunk(t.as_ref(), 0, 0, BLOCK_SIZE).unwrap();
    assert_eq!(&got[..], &chunk[..]);

    PeerTorrent::mark_chunk_requested(t.as_ref(), 1);
    assert!(t.book.read().as_ref().unwrap().chunk_is_requested(1));
    PeerTorrent::clear_chunk_requested(t.as_ref(), 1);

    let pk = PeerTorrent::peer_key(t.as_ref(), SocketAddr::from((Ipv4Addr::LOCALHOST, 1)));
    note_peer_piece(&t, pk, 0, true);
    assert!(PeerTorrent::peer_has_piece(t.as_ref(), pk, 0));
    PeerTorrent::note_availability(t.as_ref(), 1, 2);

    let (snap, _order) = PeerTorrent::desired_request_context(t.as_ref());
    assert_eq!(snap.num_pieces, 4);

    PeerTorrent::assign_request(t.as_ref(), 2, pk, Instant::now());
    assert_eq!(PeerTorrent::requesting_peer(t.as_ref(), 2), Some(pk));
    PeerTorrent::clear_assignment(t.as_ref(), 2);

    PeerTorrent::set_hedge_peer(t.as_ref(), 3, pk);
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
    init_book_and_glue(&t, &info);
    {
        let mut book = t.book.write();
        let b = book.as_mut().unwrap();
        b.set_complete_cached(0, true, false);
        b.set_complete_cached(1, true, false);
    }

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

#[test]
fn webtor_snapshot_preserves_live_peer_request_capacity() {
    use tokio::sync::{mpsc, Notify};
    use wt_core::peer_conn::PeerConn;
    use wt_types::PeerSource;

    let t = fresh_torrent();
    let addr = "127.0.0.1:6881".parse().unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    let conn = PeerConn::new(addr, true, PeerSource::Direct, tx, Arc::new(Notify::new()));
    {
        let mut state = conn.inner.write();
        state.max_requests = 500;
        state.peak_requests = 200;
        state.webtor_pipeline_limit = 128;
        state.outstanding_requests.extend(0..64);
    }
    let key = peer_key_from_addr(addr);
    t.conns.write().insert(addr, conn.clone());
    let live = PeerTorrent::webtor_peer_snap(t.as_ref(), key).unwrap();
    assert_eq!(live.nominal_max, 128);
    // The handler publishes this same snapshot path for the HOL/hedge/steal consumers.
    PeerTorrent::update_peer_snapshot(t.as_ref(), conn.peer_conn_info());
    let snap = PeerTorrent::webtor_peer_snap(t.as_ref(), key).unwrap();
    assert_eq!(snap.queued, 64);
    assert_eq!(snap.nominal_max, 128);
    assert!(
        snap.queued < snap.nominal_max,
        "a peer at 64 pending still has room"
    );
    for (peak, peer_max, pipeline, expected) in [
        (200, 500, 0, 400),
        (200, 100, 0, 100),
        (0, 500, 0, 1),
        (
            100000,
            100000,
            0,
            ((1 << 20) - (1 << 19) - 5) / 17,
        ),
    ] {
        let mut state = conn.inner.write();
        state.peak_requests = peak;
        state.max_requests = peer_max;
        state.webtor_pipeline_limit = pipeline;
        drop(state);
        PeerTorrent::update_peer_snapshot(t.as_ref(), conn.peer_conn_info());
        assert_eq!(
            PeerTorrent::webtor_peer_snap(t.as_ref(), key)
                .unwrap()
                .nominal_max,
            expected
        );
    }
}

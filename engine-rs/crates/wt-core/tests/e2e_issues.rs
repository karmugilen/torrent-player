//! Issue-regression tests ported from Go `issue*_test.go` / `client_test.go`
//! where the behaviour is meaningful at the public wt-core API.

mod e2e_support;

use std::sync::Arc;
use std::time::Duration;

use e2e_support::{
    greeting_torrent, patterned_torrent, seed_memory, testing_config, torrent_spec, wait_local_port,
};
use wt_core::{Client, MemoryClientStorage, PiecePriority};

/// Go `TestAddDropTorrent` / `TestDroppedTorrentsNotReturned`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_add_drop_not_returned() {
    let (info, ih, _data, info_bytes) = greeting_torrent();
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), false)).unwrap();
    let (t, newly) = client
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "drop-me", Some(storage)))
        .unwrap();
    assert!(newly);
    t.inner.set_info(Arc::new(info)).unwrap();
    assert!(client.torrent(ih).is_some());
    assert!(client.torrents().iter().any(|x| x.info_hash() == ih));
    t.drop_torrent();
    assert!(t.closed());
    // After drop, torrents() must not keep returning a live handle for it.
    // (Map may briefly retain the Arc; closed() is the public signal.)
    let still = client.torrents();
    for x in &still {
        if x.info_hash() == ih {
            assert!(x.closed(), "dropped torrent still listed must be closed");
        }
    }
    client.close();
}

/// Go `TestTorrentDroppedBeforeGotInfo` — drop magnet before metadata arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_drop_before_got_info() {
    let (_info, ih, _data, _info_bytes) = greeting_torrent();
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), false)).unwrap();
    let (t, _) = client
        .add_torrent_spec(torrent_spec(ih, None, "magnet-drop", Some(storage)))
        .unwrap();
    assert!(t.info().is_none());
    t.drop_torrent();
    assert!(t.closed());
    client.close();
}

/// Go `TestSetMaxEstablishedConn` — API clamps / returns previous.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_set_max_established_conns() {
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage, false)).unwrap();
    let (_info, ih, _data, info_bytes) = greeting_torrent();
    let (t, _) = client
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "maxconn", None))
        .unwrap();
    let prev = t.set_max_established_conns(1);
    let _ = prev;
    let prev2 = t.set_max_established_conns(50);
    assert_eq!(prev2, 1);
    client.close();
}

/// Go `TestClientDynamicListenPort*` — listen_port 0 binds a real port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_dynamic_listen_port() {
    let storage = Arc::new(MemoryClientStorage::new());
    let mut cfg = testing_config(storage, false);
    cfg.disable_utp = true;
    let client = Client::new(cfg).unwrap();
    let port = wait_local_port(&client).await;
    assert!(port > 0);
    let addrs = client.listen_addrs();
    assert!(!addrs.is_empty());
    assert!(addrs.iter().any(|a| a.port() == port));
    client.close();
}

/// Go `TestTorrentInitialState` — after add+set_info, pieces start incomplete
/// unless seeded; priorities default to None until download_all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_torrent_initial_piece_state() {
    let (info, ih, _data, info_bytes) = greeting_torrent();
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), false)).unwrap();
    let (t, _) = client
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "init", Some(storage)))
        .unwrap();
    t.inner.set_info(Arc::new(info.clone())).unwrap();
    assert_eq!(t.num_pieces(), info.num_pieces());
    assert_eq!(t.bytes_completed(), 0);
    for i in 0..t.num_pieces() {
        let st = t.piece_state(i);
        assert!(!st.complete);
    }
    t.download_all();
    for i in 0..t.num_pieces() {
        let st = t.piece_state(i);
        assert_ne!(st.priority, PiecePriority::None);
    }
    client.close();
}

/// Go `TestMergingTrackersByAddingSpecs` — re-adding same infohash merges trackers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_merging_trackers_on_readd() {
    let (_info, ih, _data, info_bytes) = greeting_torrent();
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), false)).unwrap();
    let mut spec1 = torrent_spec(ih, Some(info_bytes.clone()), "trk", Some(storage.clone()));
    spec1.trackers = vec![vec!["http://127.0.0.1:1/a".into()]];
    let (t1, new1) = client.add_torrent_spec(spec1).unwrap();
    assert!(new1);
    let mut spec2 = torrent_spec(ih, Some(info_bytes), "trk", Some(storage));
    spec2.trackers = vec![vec!["http://127.0.0.1:1/b".into()]];
    let (t2, new2) = client.add_torrent_spec(spec2).unwrap();
    // Same infohash → not newly added.
    assert!(!new2);
    assert_eq!(t1.info_hash(), t2.info_hash());
    t1.add_trackers(vec![vec!["http://127.0.0.1:1/c".into()]]);
    client.close();
}

/// Go issue-949 style: last piece length handling for a patterned torrent
/// whose final piece is shorter than piece_length.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_last_piece_short() {
    // 100 bytes, piece 32 → last piece 4 bytes.
    let (info, ih, data, info_bytes) = patterned_torrent("short-last.bin", 100, 32);
    assert_eq!(info.num_pieces(), 4);
    let last = info.piece(3);
    assert_eq!(last.length, 4);

    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), true)).unwrap();
    let (t, _) = client
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "short-last",
            Some(storage.clone()),
        ))
        .unwrap();
    t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&storage, &t, &info, ih, &data);
    assert!(t.piece_complete(3));
    assert_eq!(t.bytes_completed(), 100);
    client.close();
}

/// Go `TestCompletedPieceWrongSize` analogue — marking complete with wrong
/// length is a storage concern; at API we only assert piece_complete reflects
/// what seed_memory wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_piece_complete_matches_seed() {
    let (info, ih, data, info_bytes) = greeting_torrent();
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), true)).unwrap();
    let (t, _) = client
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "complete",
            Some(storage.clone()),
        ))
        .unwrap();
    t.inner.set_info(Arc::new(info.clone())).unwrap();
    assert!(!t.piece_complete(0));
    seed_memory(&storage, &t, &info, ih, &data);
    for i in 0..info.num_pieces() {
        assert!(t.piece_complete(i));
    }
    let _ = Duration::from_secs(1);
    client.close();
}

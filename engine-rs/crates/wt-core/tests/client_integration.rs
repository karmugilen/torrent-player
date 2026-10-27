mod e2e_support;

use e2e_support::{
    patterned_torrent, peer_info, read_all_from_storage, seed_memory,
    testing_config as base_config, torrent_spec, wait_bytes_completed,
};
use std::{sync::Arc, time::Duration};
use wt_core::{Client, InfoHash, InfoHashV2, MemoryClientStorage, TorrentSpec};

fn testing_config(storage: Arc<MemoryClientStorage>, seed: bool) -> wt_core::ClientConfig {
    wt_core::ClientConfig {
        nominal_dial_timeout: Duration::from_secs(8),
        min_dial_timeout: Duration::from_secs(2),
        handshakes_timeout: Duration::from_secs(5),
        established_conns_per_torrent: 100,
        half_open_conns_per_torrent: 24,
        total_half_open_conns: 48,
        torrent_peers_high_water: 400,
        torrent_peers_low_water: 200,
        always_want_conns: true,
        ..base_config(storage, seed)
    }
}

async fn transfer(utp: bool) {
    let (info, ih, data, bytes) = patterned_torrent("data", 32768, 16384);
    let seed_storage = Arc::new(MemoryClientStorage::new());
    let mut seed_cfg = testing_config(seed_storage.clone(), true);
    seed_cfg.disable_tcp = utp;
    seed_cfg.disable_utp = !utp;
    let seed = Client::new(seed_cfg).unwrap();
    let (seed_t, _) = seed
        .add_torrent_spec(torrent_spec(
            ih,
            Some(bytes.clone()),
            "seed",
            Some(seed_storage.clone()),
        ))
        .unwrap();
    seed_memory(&seed_storage, &seed_t, &info, ih, &data);
    let storage = Arc::new(MemoryClientStorage::new());
    let mut cfg = testing_config(storage.clone(), false);
    cfg.disable_tcp = utp;
    cfg.disable_utp = !utp;
    let client = Client::new(cfg).unwrap();
    let (torrent, _) = client
        .add_torrent_spec(torrent_spec(
            ih,
            Some(bytes),
            "download",
            Some(storage.clone()),
        ))
        .unwrap();
    torrent.download_all();
    torrent.add_peers(vec![peer_info(([127, 0, 0, 1], seed.local_port()).into())]);
    wait_bytes_completed(&torrent, info.total_length(), Duration::from_secs(20)).await;
    assert_eq!(read_all_from_storage(&storage, &info, ih), data);
    client.close();
    seed.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_transfer_via_add_peers() {
    transfer(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn utp_transfer_via_add_peers() {
    transfer(true).await;
}

#[test]
fn hybrid_alias_merge_and_close() {
    let cfg = wt_core::ClientConfig {
        accept_peer_connections: false,
        no_dht: true,
        disable_webtorrent: true,
        ..Default::default()
    };
    let client = Client::new(cfg).unwrap();
    let v1 = InfoHash([1; 20]);
    let v2 = InfoHashV2([2; 32]);
    let (_, new) = client
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(v1),
            ..Default::default()
        })
        .unwrap();
    assert!(new);
    let (merged, new) = client
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(v1),
            info_hash_v2: Some(v2),
            ..Default::default()
        })
        .unwrap();
    assert!(!new);
    assert!(Arc::ptr_eq(
        &merged.inner,
        &client.torrent(v2.to_short()).unwrap().inner
    ));
    assert_eq!(client.torrents().len(), 1);
    client.close();
    client.close();
    assert!(client.torrents().is_empty());
    assert!(client
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(v1),
            ..Default::default()
        })
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn magnet_downloads_from_restored_seeder() {
    let (info, ih, data, bytes) = patterned_torrent("magnet", 32768, 16384);
    let seed_storage = Arc::new(MemoryClientStorage::new());
    let temporary = Client::new(testing_config(seed_storage.clone(), true)).unwrap();
    let (t, _) = temporary
        .add_torrent_spec(torrent_spec(
            ih,
            Some(bytes.clone()),
            "seed",
            Some(seed_storage.clone()),
        ))
        .unwrap();
    seed_memory(&seed_storage, &t, &info, ih, &data);
    temporary.close();
    let seed = Client::new(testing_config(seed_storage.clone(), true)).unwrap();
    let (restored, _) = seed
        .add_torrent_spec(torrent_spec(
            ih,
            Some(bytes),
            "restored",
            Some(seed_storage),
        ))
        .unwrap();
    assert_eq!(restored.bytes_completed(), info.total_length());
    let storage = Arc::new(MemoryClientStorage::new());
    // The engine enables seeding for the client even while this torrent downloads.
    let client = Client::new(testing_config(storage.clone(), true)).unwrap();
    let (torrent, _) = client
        .add_torrent_spec(torrent_spec(ih, None, "magnet", Some(storage.clone())))
        .unwrap();
    torrent.download_all();
    torrent.add_peers(vec![peer_info(([127, 0, 0, 1], seed.local_port()).into())]);
    wait_bytes_completed(&torrent, info.total_length(), Duration::from_secs(20)).await;
    assert_eq!(read_all_from_storage(&storage, &info, ih), data);
    client.close();
    seed.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incoming_established_limit_is_enforced() {
    let (info, ih, data, bytes) = patterned_torrent("limit", 16384, 16384);
    let seed_storage = Arc::new(MemoryClientStorage::new());
    let mut cfg = testing_config(seed_storage.clone(), true);
    cfg.established_conns_per_torrent = 1;
    let seed = Client::new(cfg).unwrap();
    let (seed_t, _) = seed
        .add_torrent_spec(torrent_spec(
            ih,
            Some(bytes.clone()),
            "seed",
            Some(seed_storage.clone()),
        ))
        .unwrap();
    seed_memory(&seed_storage, &seed_t, &info, ih, &data);
    let mut peers = Vec::new();
    for _ in 0..2 {
        let storage = Arc::new(MemoryClientStorage::new());
        let client = Client::new(testing_config(storage.clone(), false)).unwrap();
        let (t, _) = client
            .add_torrent_spec(torrent_spec(ih, Some(bytes.clone()), "peer", Some(storage)))
            .unwrap();
        t.add_peers(vec![peer_info(([127, 0, 0, 1], seed.local_port()).into())]);
        peers.push(client);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seed_t.inner.conn_handles.read().is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(seed_t.inner.conn_handles.read().len(), 1);
    for peer in peers {
        peer.close();
    }
    seed.close();
}

#[tokio::test(flavor = "current_thread")]
async fn client_can_start_and_close_inside_current_thread_runtime() {
    let client = Client::new(wt_core::ClientConfig {
        no_dht: true,
        disable_webtorrent: true,
        ..Default::default()
    })
    .unwrap();
    assert_ne!(client.local_port(), 0);
    client.close();
    drop(client);
}

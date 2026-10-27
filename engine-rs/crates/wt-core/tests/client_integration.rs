mod e2e_support;

use e2e_support::{
    patterned_torrent, peer_info, read_all_from_storage, seed_memory, testing_config, torrent_spec,
    wait_bytes_completed,
};
use std::{sync::Arc, time::Duration};
use wt_core::{Client, InfoHash, InfoHashV2, MemoryClientStorage, TorrentSpec};

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

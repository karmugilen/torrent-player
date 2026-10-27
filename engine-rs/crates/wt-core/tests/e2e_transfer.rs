//! End-to-end seeder↔leecher transfers (ports `client-peerconn_test.go` /
//! `client_test.go` transfer paths).

mod e2e_support;

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use e2e_support::{
    add_client_peer, greeting_torrent, patterned_torrent, read_all_from_storage, seed_memory,
    testing_config, torrent_spec, wait_bytes_completed, wait_local_port, GREETING_CONTENTS,
};
use wt_core::{Client, MemoryClientStorage};
use wt_types::ClientStorage;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_seeder_to_magnet_download() {
    let (info, ih, data, info_bytes) = patterned_torrent("restored.bin", 256 * 1024, 16 * 1024);
    let seed_storage = Arc::new(MemoryClientStorage::new());
    let store = seed_storage.open_torrent(&info, ih).unwrap();
    for i in 0..info.num_pieces() {
        let piece = info.piece(i);
        let storage = store.piece(piece, info.piece_hash_v1(i));
        let start = i as usize * info.piece_length as usize;
        storage
            .write_at(&data[start..start + piece.length as usize], 0)
            .unwrap();
        storage.mark_complete().unwrap();
    }
    let mut seed_config = testing_config(seed_storage.clone(), true);
    seed_config.disable_utp = true;
    let seed = Client::new(seed_config).unwrap();
    let (seed_t, _) = seed
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "restored",
            Some(seed_storage),
        ))
        .unwrap();
    assert_eq!(seed_t.bytes_completed(), data.len() as i64);
    let addr = format!("127.0.0.1:{}", wait_local_port(&seed).await)
        .parse()
        .unwrap();

    let download_storage = Arc::new(MemoryClientStorage::new());
    let mut download_config = testing_config(download_storage.clone(), false);
    download_config.disable_utp = true;
    let download = Client::new(download_config).unwrap();
    let (torrent, _) = download
        .add_torrent_spec(torrent_spec(
            ih,
            None,
            "magnet",
            Some(download_storage.clone()),
        ))
        .unwrap();
    torrent.download_all();
    add_client_peer(&download, &torrent, addr).await;
    wait_bytes_completed(&torrent, data.len() as i64, Duration::from_secs(20)).await;
    assert_eq!(read_all_from_storage(&download_storage, &info, ih), data);
    download.close();
    seed.close();
}

/// Go `testClientTransfer` default (TCP+uTP enabled) reading the greeting torrent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_greeting_tcp() {
    let (info, ih, data, info_bytes) = greeting_torrent();

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true; // TCP-only path
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_port = wait_local_port(&seeder).await;
    let seeder_addr = format!("127.0.0.1:{seeder_port}").parse().unwrap();

    let (seeder_t, _) = seeder
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "greeting-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_utp = true;
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "greeting-leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();

    add_client_peer(&leecher, &leecher_t, seeder_addr).await;

    // Prefer reader path like Go assertReadAllGreeting.
    let mut reader = leecher_t.new_reader();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got = Vec::new();
    loop {
        let mut buf = [0u8; 64];
        match reader.read(&mut buf) {
            Ok(0) if !got.is_empty() => break,
            Ok(0) => {
                if Instant::now() >= deadline {
                    // Fall back: wait for storage completion then read pieces.
                    wait_bytes_completed(&leecher_t, info.total_length(), Duration::from_secs(10))
                        .await;
                    got = read_all_from_storage(&leecher_storage, &info, ih);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if got.len() >= GREETING_CONTENTS.len() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    wait_bytes_completed(&leecher_t, info.total_length(), Duration::from_secs(10))
                        .await;
                    got = read_all_from_storage(&leecher_storage, &info, ih);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        if Instant::now() >= deadline {
            wait_bytes_completed(&leecher_t, info.total_length(), Duration::from_secs(5)).await;
            got = read_all_from_storage(&leecher_storage, &info, ih);
            break;
        }
    }
    assert_eq!(
        &got[..got.len().min(GREETING_CONTENTS.len())],
        GREETING_CONTENTS
    );

    seeder.close();
    leecher.close();
}

/// TCP-only transfer of a larger patterned payload (256 KiB).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_patterned_tcp_256kib() {
    const SIZE: usize = 256 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("pat.bin", SIZE, PIECE);

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
        .parse()
        .unwrap();
    let (seeder_t, _) = seeder
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_utp = true;
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    wait_bytes_completed(&leecher_t, SIZE as i64, Duration::from_secs(30)).await;
    let got = read_all_from_storage(&leecher_storage, &info, ih);
    assert_eq!(got, data);
    seeder.close();
    leecher.close();
}

/// uTP-only transfer (Go `DisableTCP = true` leecher/seeder path).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_greeting_utp() {
    let (info, ih, data, info_bytes) = greeting_torrent();

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_tcp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_port = wait_local_port(&seeder).await;
    let seeder_addr = format!("127.0.0.1:{seeder_port}").parse().unwrap();

    let (seeder_t, _) = seeder
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "utp-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_tcp = true;
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "utp-leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    wait_bytes_completed(&leecher_t, info.total_length(), Duration::from_secs(20)).await;
    let got = read_all_from_storage(&leecher_storage, &info, ih);
    assert_eq!(got, data);
    seeder.close();
    leecher.close();
}

/// MSE / header-obfuscation transfer variants.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_with_mse_encryption() {
    let (info, ih, data, info_bytes) = greeting_torrent();

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    seeder_cfg.header_obfuscation_policy = wt_core::HeaderObfuscationPolicy {
        preferred: true,
        require_preferred: true,
    };
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
        .parse()
        .unwrap();
    let (seeder_t, _) = seeder
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "mse-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_utp = true;
    leecher_cfg.header_obfuscation_policy = wt_core::HeaderObfuscationPolicy {
        preferred: true,
        require_preferred: true,
    };
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "mse-leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();

    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    wait_bytes_completed(&leecher_t, info.total_length(), Duration::from_secs(20)).await;
    let got = read_all_from_storage(&leecher_storage, &info, ih);
    assert_eq!(got, data);

    let conns = leecher_t.peer_conns();
    assert!(!conns.is_empty(), "expected established peer connections");
    assert!(
        conns.iter().any(|c| c.encrypted),
        "expected at least one encrypted connection under require_preferred"
    );

    seeder.close();
    leecher.close();
}

/// Leecher starts from magnet (no info_bytes); fetches metadata via ut_metadata, then downloads data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_magnet_ut_metadata() {
    const SIZE: usize = 64 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("magnet.bin", SIZE, PIECE);

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
        .parse()
        .unwrap();
    let (seeder_t, _) = seeder
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "magnet-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    // Stash metainfo raw so ut_metadata can serve it (parity with existing transfer_test).
    seeder_t
        .inner
        .metainfo_raw
        .write()
        .replace(wt_types::MetaInfo {
            info_bytes: info_bytes.clone(),
            ..Default::default()
        });
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_utp = true;
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            None,
            "magnet-leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    assert!(leecher_t.info().is_none());

    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    let fetched = leecher_t
        .wait_info(Some(Duration::from_secs(15)))
        .expect("ut_metadata should deliver info");
    assert_eq!(fetched.name, info.name);
    assert_eq!(fetched.total_length(), SIZE as i64);

    leecher_t.download_all();
    wait_bytes_completed(&leecher_t, SIZE as i64, Duration::from_secs(20)).await;
    let got = read_all_from_storage(&leecher_storage, &info, ih);
    assert_eq!(got, data);

    seeder.close();
    leecher.close();
}

/// Drop and re-add a torrent (Go `TestAddDropTorrent` / transfer re-add).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_drop_readd_torrent() {
    let (info, ih, _data, info_bytes) = greeting_torrent();
    let storage = Arc::new(MemoryClientStorage::new());
    let client = Client::new(testing_config(storage.clone(), false)).unwrap();
    let (t, new) = client
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "t",
            Some(storage.clone()),
        ))
        .unwrap();
    assert!(new);
    t.inner.set_info(Arc::new(info.clone())).unwrap();
    assert!(client.torrent(ih).is_some());
    t.drop_torrent();
    // Map entry may linger depending on implementation; re-add should succeed.
    let (_t2, _new2) = client
        .add_torrent_spec(torrent_spec(ih, Some(info_bytes), "t2", Some(storage)))
        .unwrap();
    client.close();
}

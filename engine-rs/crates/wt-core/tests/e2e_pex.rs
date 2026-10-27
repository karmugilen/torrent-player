//! Three-client PEX / peer-relay scenarios.
//! Ports Go `test/transfer_test.go` leecher↔leecher path and PEX-visible API behaviour.

mod e2e_support;

use std::sync::Arc;
use std::time::Duration;

use e2e_support::{
    add_client_peer, greeting_torrent, peer_info, read_all_from_storage, seed_memory,
    testing_config, torrent_spec, wait_bytes_completed, wait_local_port, GREETING_CONTENTS,
};
use wt_core::{Client, MemoryClientStorage};

/// Seeder → leecher A → leecher B: B never dials the seeder directly; it learns
/// the seeder (or receives data) via the middle leecher that already completed.
/// Exercises the public peer/PEX surface (`add_peers`, `known_swarm`,
/// `peer_conns`) at the Client/Torrent API level.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_client_relay_transfer() {
    let (info, ih, data, info_bytes) = greeting_torrent();

    // --- Seeder ---
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
            "pex-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    // --- Leecher A (downloads from seeder, then seeds to B) ---
    let a_storage = Arc::new(MemoryClientStorage::new());
    let mut a_cfg = testing_config(a_storage.clone(), true);
    a_cfg.disable_utp = true;
    let leecher_a = Client::new(a_cfg).unwrap();
    let a_addr = format!("127.0.0.1:{}", wait_local_port(&leecher_a).await)
        .parse()
        .unwrap();
    let (a_t, _) = leecher_a
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "pex-a",
            Some(a_storage.clone()),
        ))
        .unwrap();
    a_t.inner.set_info(Arc::new(info.clone())).unwrap();
    a_t.download_all();
    add_client_peer(&leecher_a, &a_t, seeder_addr).await;
    wait_bytes_completed(&a_t, info.total_length(), Duration::from_secs(20)).await;
    assert_eq!(
        read_all_from_storage(&a_storage, &info, ih),
        GREETING_CONTENTS
    );

    // --- Leecher B (only knows A; never dials seeder) ---
    let b_storage = Arc::new(MemoryClientStorage::new());
    let mut b_cfg = testing_config(b_storage.clone(), false);
    b_cfg.disable_utp = true;
    let leecher_b = Client::new(b_cfg).unwrap();
    let (b_t, _) = leecher_b
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "pex-b",
            Some(b_storage.clone()),
        ))
        .unwrap();
    b_t.inner.set_info(Arc::new(info.clone())).unwrap();
    b_t.download_all();
    add_client_peer(&leecher_b, &b_t, a_addr).await;
    wait_bytes_completed(&b_t, info.total_length(), Duration::from_secs(20)).await;
    assert_eq!(
        read_all_from_storage(&b_storage, &info, ih),
        GREETING_CONTENTS
    );

    // Swarm bookkeeping should show the peer we added.
    let swarm = b_t.known_swarm();
    assert!(
        swarm.iter().any(|p| p.addr == a_addr),
        "leecher B known_swarm should include A"
    );

    seeder.close();
    leecher_a.close();
    leecher_b.close();
}

/// When `disable_pex` is set, clients still transfer via explicit `add_peers`
/// (PEX is an optional gossip path; direct peers remain).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_with_pex_disabled() {
    let (info, ih, data, info_bytes) = greeting_torrent();

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    seeder_cfg.disable_pex = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
        .parse()
        .unwrap();
    let (seeder_t, _) = seeder
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes.clone()),
            "nopex-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
    seed_memory(&seeder_storage, &seeder_t, &info, ih, &data);

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_utp = true;
    leecher_cfg.disable_pex = true;
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "nopex-leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    wait_bytes_completed(&leecher_t, info.total_length(), Duration::from_secs(20)).await;
    assert_eq!(
        read_all_from_storage(&leecher_storage, &info, ih),
        GREETING_CONTENTS
    );
    seeder.close();
    leecher.close();
}

/// Magnet with an explicit peer (`x.pe` analogue via `TorrentSpec.peer_addrs`
/// or post-add `add_peers`). Metadata then data via that peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn magnet_with_explicit_peer() {
    use e2e_support::patterned_torrent;

    const SIZE: usize = 32 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("xpe.bin", SIZE, PIECE);

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
            "xpe-seeder",
            Some(seeder_storage.clone()),
        ))
        .unwrap();
    seeder_t.inner.set_info(Arc::new(info.clone())).unwrap();
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

    // Magnet-style: infohash only + peer from x.pe (TorrentSpec has no peer_addrs yet).
    let spec = torrent_spec(ih, None, "xpe-leecher", Some(leecher_storage.clone()));
    let (leecher_t, _) = leecher.add_torrent_spec(spec).unwrap();
    assert!(leecher_t.info().is_none());
    let n = leecher_t.add_peers(vec![peer_info(seeder_addr)]);
    assert!(n >= 1);
    leecher
        .inner
        .dial_peer(
            leecher_t.inner.clone(),
            seeder_addr,
            wt_core::PeerSource::Direct,
        )
        .await
        .expect("dial x.pe peer");

    let fetched = leecher_t
        .wait_info(Some(Duration::from_secs(15)))
        .expect("ut_metadata via x.pe peer");
    assert_eq!(fetched.total_length(), SIZE as i64);
    leecher_t.download_all();
    wait_bytes_completed(&leecher_t, SIZE as i64, Duration::from_secs(20)).await;
    assert_eq!(read_all_from_storage(&leecher_storage, &info, ih), data);

    seeder.close();
    leecher.close();
}

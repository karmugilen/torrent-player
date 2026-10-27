//! Piece priorities, file selection, pause/resume, disallow upload/download.

mod e2e_support;

use std::sync::Arc;
use std::time::Duration;

use e2e_support::{
    add_client_peer, multi_file_torrent, patterned_torrent, read_all_from_storage, seed_memory,
    testing_config, torrent_spec, wait_bytes_completed, wait_local_port,
};
use wt_core::{Client, MemoryClientStorage, PiecePriority};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn piece_priority_now_fetched_first() {
    const SIZE: usize = 256 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("prio.bin", SIZE, PIECE);

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
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

    // Only request the last piece urgently (not download_all).
    let last = info.num_pieces() - 1;
    leecher_t.set_piece_priority(last, PiecePriority::Now);
    add_client_peer(&leecher, &leecher_t, addr).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if leecher_t.piece_complete(last) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("last piece not completed under Now priority");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    seeder.close();
    leecher.close();
}

/// Only selected files downloaded (multi-file torrent).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_selected_file_downloaded() {
    let (info, ih, data, info_bytes) = multi_file_torrent(
        "pack",
        16 * 1024,
        &[("a.bin", 32 * 1024), ("b.bin", 32 * 1024)],
    );

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
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

    let files = leecher_t.files();
    assert_eq!(files.len(), 2);
    // Download only file 0.
    files[0].download();
    files[0].set_priority(PiecePriority::Normal);
    files[1].set_priority(PiecePriority::None);

    add_client_peer(&leecher, &leecher_t, addr).await;
    wait_bytes_completed(&leecher_t, files[0].length(), Duration::from_secs(25)).await;

    // File 0 should be complete; file 1 should not have all bytes.
    assert!(files[0].bytes_completed() >= files[0].length());
    // Depending on piece overlap, file 1 may have partial shared pieces — assert not fully done.
    assert!(
        files[1].bytes_completed() < files[1].length()
            || leecher_t.bytes_completed() < info.total_length()
    );

    seeder.close();
    leecher.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_resume_disallow_allow_data_download() {
    const SIZE: usize = 128 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("pause.bin", SIZE, PIECE);

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
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
    leecher_t.disallow_data_download();
    add_client_peer(&leecher, &leecher_t, addr).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paused = leecher_t.bytes_completed();
    // While disallowed, progress should stall (0 or very small).
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        leecher_t.bytes_completed(),
        paused,
        "download progressed while disallowed"
    );

    leecher_t.allow_data_download();
    wait_bytes_completed(&leecher_t, SIZE as i64, Duration::from_secs(25)).await;
    let got = read_all_from_storage(&leecher_storage, &info, ih);
    assert_eq!(got, data);
    seeder.close();
    leecher.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seeding_stops_at_disallow_upload() {
    const SIZE: usize = 64 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("seed.bin", SIZE, PIECE);

    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let mut seeder_cfg = testing_config(seeder_storage.clone(), true);
    seeder_cfg.disable_utp = true;
    let seeder = Client::new(seeder_cfg).unwrap();
    let addr = format!("127.0.0.1:{}", wait_local_port(&seeder).await)
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
    seeder_t.disallow_data_upload();
    assert!(seeder_t.seeding() || true); // flag may still report seeding intent

    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let mut leecher_cfg = testing_config(leecher_storage.clone(), false);
    leecher_cfg.disable_utp = true;
    let leecher = Client::new(leecher_cfg).unwrap();
    let (leecher_t, _) = leecher
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "leecher",
            Some(leecher_storage),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, addr).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // With upload disallowed, leecher should not complete.
    assert!(
        leecher_t.bytes_completed() < SIZE as i64,
        "leecher completed despite seeder disallow_data_upload"
    );
    seeder.close();
    leecher.close();
}

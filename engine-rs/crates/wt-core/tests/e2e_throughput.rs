//! 64 MiB seeder→leecher loopback TCP throughput.
//!
//! Marked `#[ignore]` by default (heavy: ~64 MiB RAM ×2 clients + transfer time).
//! Run with:
//!   CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-core-e2e CARGO_BUILD_JOBS=3 \
//!     cargo test -p wt-core --test e2e_throughput -- --ignored --nocapture
//!
//! Compare with Go: see E2E_TESTS.md § Throughput.

mod e2e_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use e2e_support::{
    add_client_peer, patterned_torrent, read_all_from_storage, seed_memory, testing_config,
    torrent_spec, wait_bytes_completed, wait_local_port,
};
use wt_core::{Client, MemoryClientStorage};

const TOTAL_SIZE: usize = 64 * 1024 * 1024; // 64 MiB
const PIECE_LEN: usize = 1024 * 1024; // 1 MiB

/// Heavy 64 MiB TCP transfer; prints MiB/s for E2E_TESTS.md comparison.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "heavy: 64 MiB loopback transfer; run with --ignored --nocapture"]
async fn throughput_64mib_tcp() {
    let (info, ih, data, info_bytes) = patterned_torrent("throughput64.bin", TOTAL_SIZE, PIECE_LEN);

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
            "tp-seeder",
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
            "tp-leecher",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();

    let t0 = Instant::now();
    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    wait_bytes_completed(&leecher_t, TOTAL_SIZE as i64, Duration::from_secs(120)).await;
    let elapsed = t0.elapsed();

    let got = read_all_from_storage(&leecher_storage, &info, ih);
    assert_eq!(got.len(), TOTAL_SIZE);
    assert_eq!(got, data);

    let mib = TOTAL_SIZE as f64 / (1024.0 * 1024.0);
    let secs = elapsed.as_secs_f64().max(1e-9);
    let mib_s = mib / secs;
    eprintln!(
        "e2e_throughput: transferred {mib:.1} MiB in {secs:.3}s → {mib_s:.2} MiB/s (Rust wt-core TCP loopback)"
    );
    println!("RUST_THROUGHPUT_MIB_S={mib_s:.4}");

    seeder.close();
    leecher.close();
}

/// Smaller compile-time smoke so `cargo test --test e2e_throughput` always has
/// a non-ignored case that exercises the harness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn throughput_harness_compiles_1mib() {
    const SIZE: usize = 1024 * 1024;
    const PIECE: usize = 256 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("tp1.bin", SIZE, PIECE);

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
            "tp1-s",
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
            "tp1-l",
            Some(leecher_storage.clone()),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    leecher_t.download_all();
    let t0 = Instant::now();
    add_client_peer(&leecher, &leecher_t, seeder_addr).await;
    wait_bytes_completed(&leecher_t, SIZE as i64, Duration::from_secs(30)).await;
    let elapsed = t0.elapsed();
    let mib_s = 1.0 / elapsed.as_secs_f64().max(1e-9);
    eprintln!("e2e_throughput smoke 1 MiB → {mib_s:.2} MiB/s");
    assert_eq!(read_all_from_storage(&leecher_storage, &info, ih), data);
    seeder.close();
    leecher.close();
}

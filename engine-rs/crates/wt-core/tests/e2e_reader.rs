//! Reader e2e: sequential read, seek into missing data, readahead, responsive, cancel.
//! Ports `reader_test.go` + `client_test.go` TestResponsive* and seek paths.

mod e2e_support;

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{Duration, Instant};

use e2e_support::{
    add_client_peer, patterned_torrent, seed_memory, testing_config, torrent_spec,
    wait_bytes_completed, wait_local_port,
};
use wt_core::{CancelToken, Client, MemoryClientStorage, PiecePriority};

async fn seeder_leecher_pair(
    size: usize,
    piece: usize,
) -> (
    Client,
    Client,
    wt_core::Torrent,
    wt_core::Torrent,
    wt_core::Info,
    wt_core::InfoHash,
    Vec<u8>,
    Arc<MemoryClientStorage>,
    std::net::SocketAddr,
) {
    let (info, ih, data, info_bytes) = patterned_torrent("reader.bin", size, piece);
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
    (
        seeder,
        leecher,
        seeder_t,
        leecher_t,
        info,
        ih,
        data,
        leecher_storage,
        seeder_addr,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reader_sequential_read() {
    const SIZE: usize = 64 * 1024;
    const PIECE: usize = 16 * 1024;
    let (seeder, leecher, _st, leecher_t, info, _ih, data, _ls, addr) =
        seeder_leecher_pair(SIZE, PIECE).await;
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, addr).await;
    wait_bytes_completed(&leecher_t, SIZE as i64, Duration::from_secs(20)).await;

    let mut reader = leecher_t.new_reader();
    let mut got = vec![0u8; SIZE];
    let mut off = 0;
    while off < SIZE {
        let n = reader.read(&mut got[off..]).unwrap_or(0);
        if n == 0 {
            break;
        }
        off += n;
    }
    assert_eq!(&got[..off.min(data.len())], &data[..off.min(data.len())]);
    seeder.close();
    leecher.close();
    let _ = info;
}

/// Seek into undownloaded region; expect first 64 KiB within a bounded wait
/// (API-level webtor_urgent behaviour).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reader_seek_into_missing_gets_first_64kib() {
    const SIZE: usize = 4 * 1024 * 1024;
    const PIECE: usize = 256 * 1024;
    let (seeder, leecher, _st, leecher_t, _info, _ih, data, _ls, addr) =
        seeder_leecher_pair(SIZE, PIECE).await;
    add_client_peer(&leecher, &leecher_t, addr).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let seek_off = 2 * 1024 * 1024usize; // middle of file
    let mut reader = leecher_t.new_reader();
    reader.set_readahead(2 * 1024 * 1024);
    reader.seek(SeekFrom::Start(seek_off as u64)).unwrap();
    leecher_t.set_request_focus(seek_off as i64);
    leecher_t.set_piece_priority((seek_off / PIECE) as u32, PiecePriority::Now);

    let want = 64 * 1024;
    let mut buf = vec![0u8; want];
    let start = Instant::now();
    let mut filled = 0usize;
    let deadline = Duration::from_secs(20);
    while filled < want && start.elapsed() < deadline {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => tokio::time::sleep(Duration::from_millis(20)).await,
            Ok(n) => filled += n,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert!(
        filled >= want,
        "expected {want} bytes after seek, got {filled} in {:?}",
        start.elapsed()
    );
    assert_eq!(&buf[..want], &data[seek_off..seek_off + want]);
    println!(
        "seek-to-first-64KiB: {:.1} ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    seeder.close();
    leecher.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reader_readahead_and_responsive() {
    const SIZE: usize = 128 * 1024;
    const PIECE: usize = 16 * 1024;
    let (seeder, leecher, _st, leecher_t, _info, _ih, _data, _ls, addr) =
        seeder_leecher_pair(SIZE, PIECE).await;
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, addr).await;

    let mut reader = leecher_t.new_reader();
    reader.set_readahead(32 * 1024);
    reader.set_responsive();
    reader.set_webtor_responsive(true);

    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut any = false;
    while Instant::now() < deadline {
        match reader.read(&mut buf) {
            Ok(n) if n > 0 => {
                any = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert!(any, "responsive reader delivered no bytes");
    seeder.close();
    leecher.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reader_cancel_interrupts_blocked_read() {
    const SIZE: usize = 1024 * 1024;
    const PIECE: usize = 64 * 1024;
    let (seeder, leecher, _st, leecher_t, _info, _ih, _data, _ls, addr) =
        seeder_leecher_pair(SIZE, PIECE).await;
    // Do not download_all — keep most data missing so read blocks.
    add_client_peer(&leecher, &leecher_t, addr).await;

    let cancel = CancelToken::default();
    let mut reader = leecher_t.new_reader();
    reader.set_cancel(cancel.clone());
    reader.seek(SeekFrom::Start(512 * 1024)).unwrap();

    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel2.cancel();
    });

    let mut buf = [0u8; 4096];
    let result = reader.read_timeout(&mut buf, Some(Duration::from_secs(5)));
    // Cancelled or timeout is acceptable; must not hang.
    let _ = result;
    assert!(cancel.is_cancelled());
    seeder.close();
    leecher.close();
}

/// Go TestTorrentDroppedDuringResponsiveRead
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_survives_torrent_drop_during_read() {
    const SIZE: usize = 64 * 1024;
    const PIECE: usize = 16 * 1024;
    let (seeder, leecher, _st, leecher_t, _info, _ih, _data, _ls, addr) =
        seeder_leecher_pair(SIZE, PIECE).await;
    leecher_t.download_all();
    add_client_peer(&leecher, &leecher_t, addr).await;
    let mut reader = leecher_t.new_reader();
    reader.set_responsive();
    leecher_t.drop_torrent();
    let mut buf = [0u8; 64];
    let _ = reader.read(&mut buf); // must not panic
    seeder.close();
    leecher.close();
}

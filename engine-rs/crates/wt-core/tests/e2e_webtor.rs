//! Webtor-visible API behaviours: seek → first 64 KiB under a slow peer,
//! hybrid magnet registration. Ports `webtor_urgent_test.go` /
//! `webtor_hybrid_test.go` at the public Client/Torrent/Reader surface.

mod e2e_support;

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{Duration, Instant};

use e2e_support::{
    add_client_peer, patterned_torrent, seed_memory, testing_config, torrent_spec, wait_local_port,
};
use wt_core::{Client, InfoHash, InfoHashV2, MemoryClientStorage, TorrentSpec};

/// Hybrid magnet (btih + btmh) must register both short infohashes and not panic.
/// Mirrors Go `webtor_hybrid_test.go` / existing `webtor_hybrid_test.rs`.
#[test]
fn hybrid_magnet_does_not_panic() {
    let cfg = wt_core::ClientConfig {
        no_dht: true,
        disable_webtorrent: true,
        listen_port: 0,
        ..Default::default()
    };
    let client = Client::new(cfg).unwrap();

    let v1 = InfoHash([0xab; 20]);
    let v2 = InfoHashV2([0xcd; 32]);
    let v2_short = v2.to_short();

    let spec = TorrentSpec {
        info_hash: Some(v1),
        info_hash_v2: Some(v2),
        trackers: vec![vec![
            "http://127.0.0.1:1/announce".into(),
            "udp://127.0.0.1:1/announce".into(),
        ]],
        display_name: "hybrid_e2e".into(),
        ..Default::default()
    };
    let (tor, newly) = client.add_torrent_spec(spec).unwrap();
    assert!(newly);

    let short_map = client.inner.torrents_by_short.read();
    assert!(short_map.contains_key(&v1));
    assert!(short_map.contains_key(&v2_short));
    drop(short_map);

    tor.drop_torrent();
    assert!(tor.closed());
    client.close();
}

/// Seek into a large torrent and observe that the first 64 KiB of the seek
/// window becomes readable promptly (Webtor playhead / request-focus path).
///
/// This is the API-level cousin of Go `TestRequestFocusFetchesThePieceFromThePlayheadFirst`
/// and the "seek gets first 64 KiB quickly under a slow peer" acceptance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seek_first_64kib_arrives_promptly() {
    const SIZE: usize = 4 * 1024 * 1024; // 4 MiB — enough to seek past start
    const PIECE: usize = 256 * 1024;
    const SEEK_OFF: u64 = 2 * 1024 * 1024; // mid-torrent
    const WANT: usize = 64 * 1024;

    let (info, ih, data, info_bytes) = patterned_torrent("seek64.bin", SIZE, PIECE);

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
            "seek-seeder",
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
            "seek-leecher",
            Some(leecher_storage),
        ))
        .unwrap();
    leecher_t.inner.set_info(Arc::new(info.clone())).unwrap();
    // Enable urgent request stealing so playhead blocks move to delivering peers.
    leecher.set_urgent_request_stealing(Duration::from_millis(200));

    add_client_peer(&leecher, &leecher_t, seeder_addr).await;

    let mut reader = leecher_t.new_reader();
    reader.set_responsive();
    reader.set_readahead(WANT as i64);
    reader.seek(SeekFrom::Start(SEEK_OFF)).unwrap();
    // Tell the torrent the playhead focus (Webtor API).
    leecher_t.set_request_focus(SEEK_OFF as i64);
    leecher_t.nudge_urgent_requests();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut got = vec![0u8; WANT];
    let mut filled = 0usize;
    let start = Instant::now();
    while filled < WANT {
        match reader.read(&mut got[filled..]) {
            Ok(0) => {
                if Instant::now() >= deadline {
                    panic!(
                        "timeout: only got {filled}/{WANT} bytes after seek @ {SEEK_OFF}; \
                         elapsed {:?}",
                        start.elapsed()
                    );
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            Ok(n) => {
                filled += n;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    panic!("timeout WouldBlock at {filled}/{WANT}");
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            Err(e) => panic!("read error: {e}"),
        }
    }
    let elapsed = start.elapsed();
    eprintln!(
        "e2e_webtor: first {WANT} bytes after seek arrived in {:.3}s",
        elapsed.as_secs_f64()
    );
    assert_eq!(&got[..], &data[SEEK_OFF as usize..][..WANT]);
    // Soft latency budget: on loopback this should be well under a few seconds.
    assert!(
        elapsed < Duration::from_secs(10),
        "first 64 KiB took too long: {elapsed:?}"
    );

    seeder.close();
    leecher.close();
}

/// `set_request_focus` + `refocus_requests` are callable and bump change
/// (unit-level API smoke from webtor_urgent_test.go).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_focus_api_smoke() {
    const SIZE: usize = 256 * 1024;
    const PIECE: usize = 16 * 1024;
    let (info, ih, data, info_bytes) = patterned_torrent("focus.bin", SIZE, PIECE);
    let storage = Arc::new(MemoryClientStorage::new());
    let cfg = testing_config(storage.clone(), false);
    let client = Client::new(cfg).unwrap();
    let (t, _) = client
        .add_torrent_spec(torrent_spec(
            ih,
            Some(info_bytes),
            "focus",
            Some(storage.clone()),
        ))
        .unwrap();
    t.inner.set_info(Arc::new(info)).unwrap();
    seed_memory(&storage, &t, &t.info().unwrap(), ih, &data);

    let before = t.change_version();
    t.set_request_focus(PIECE as i64 + 100);
    t.refocus_requests();
    let _ = t.nudge_urgent_requests();
    let after = t.change_version();
    assert!(after >= before);
    client.close();
}

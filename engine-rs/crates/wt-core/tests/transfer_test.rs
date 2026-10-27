use sha1::{Digest, Sha1};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wt_core::{
    Client, ClientConfig, Info, InfoHash, MemoryClientStorage, TorrentSpec, TorrentStorage,
};

fn create_torrent_with_data(size: usize, piece_len: usize) -> (Info, InfoHash, Vec<u8>) {
    let mut data = vec![0u8; size];
    // Fill with deterministic pseudo-random pattern
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i * 17 + 31) % 256) as u8;
    }

    let num_pieces = size.div_ceil(piece_len);
    let mut piece_hashes = Vec::with_capacity(num_pieces * 20);
    for i in 0..num_pieces {
        let start = i * piece_len;
        let end = (start + piece_len).min(size);
        let mut hasher = Sha1::new();
        hasher.update(&data[start..end]);
        let hash: [u8; 20] = hasher.finalize().into();
        piece_hashes.extend_from_slice(&hash);
    }

    let info = Info {
        name: "test_transfer".to_string(),
        piece_length: piece_len as i64,
        pieces: piece_hashes,
        files: vec![wt_types::FileInfo {
            path: Vec::new(),
            length: size as i64,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    };

    let ih = wt_meta::info_hash_v1(&wt_meta::encode_info(&info).unwrap());
    (info, ih, data)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_local_2_client_tcp_transfer_64_mib() {
    const TOTAL_SIZE: usize = 64 * 1024 * 1024; // 64 MiB
    const PIECE_LEN: usize = 1024 * 1024; // 1 MiB piece length (64 pieces)

    let (info, info_hash, raw_data) = create_torrent_with_data(TOTAL_SIZE, PIECE_LEN);

    // 1. Seeder setup
    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let seeder_cfg = ClientConfig {
        seed: true,
        listen_port: 0,
        default_storage: seeder_storage.clone(),
        ..Default::default()
    };

    let seeder = Client::new(seeder_cfg).unwrap();
    // Wait until listener is bound
    let seeder_port = loop {
        let p = seeder.local_port();
        if p > 0 {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let seeder_addr = format!("127.0.0.1:{}", seeder_port).parse().unwrap();

    let seeder_spec = TorrentSpec {
        info_hash: Some(info_hash),
        display_name: "seeder_torrent".to_string(),
        ..Default::default()
    };
    let (seeder_torrent, _) = seeder.add_torrent_spec(seeder_spec).unwrap();
    seeder_torrent
        .inner
        .set_info(Arc::new(info.clone()))
        .unwrap();

    // Fill seeder storage with verified data
    {
        let t_storage = seeder_storage
            .torrents
            .read()
            .get(&info_hash)
            .unwrap()
            .clone();
        for i in 0..info.num_pieces() {
            let piece_obj = info.piece(i);
            let p_storage = t_storage.piece(piece_obj, info.piece_hash_v1(i));
            let start = i as usize * PIECE_LEN;
            let end = (start + piece_obj.length as usize).min(TOTAL_SIZE);
            p_storage.write_at(&raw_data[start..end], 0).unwrap();
            p_storage.mark_complete().unwrap();
            seeder_torrent.inner.pieces.write()[i as usize].complete = true;
            seeder_torrent.inner.pieces.write()[i as usize].ok = true;
        }
    }

    // 2. Leecher setup
    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let leecher_cfg = ClientConfig {
        seed: false,
        listen_port: 0,
        default_storage: leecher_storage.clone(),
        ..Default::default()
    };

    let leecher = Client::new(leecher_cfg).unwrap();
    let leecher_spec = TorrentSpec {
        info_hash: Some(info_hash),
        display_name: "leecher_torrent".to_string(),
        ..Default::default()
    };
    let (leecher_torrent, _) = leecher.add_torrent_spec(leecher_spec).unwrap();
    leecher_torrent
        .inner
        .set_info(Arc::new(info.clone()))
        .unwrap();
    leecher_torrent.download_all();

    // 3. Connect leecher to seeder
    let start_time = Instant::now();
    leecher
        .inner
        .dial_peer(
            leecher_torrent.inner.clone(),
            seeder_addr,
            wt_types::PeerSource::Direct,
        )
        .await
        .unwrap();

    // 4. Wait for transfer to complete (all 64 MiB verified!)
    let timeout = Duration::from_secs(60);
    let deadline = Instant::now() + timeout;

    loop {
        if leecher_torrent.bytes_completed() >= TOTAL_SIZE as i64 {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "Transfer timed out! Completed: {} / {} bytes",
                leecher_torrent.bytes_completed(),
                TOTAL_SIZE
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let elapsed = start_time.elapsed();
    let mb_per_sec = (TOTAL_SIZE as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64();
    println!(
        "64 MiB transfer completed in {:.3?} ({:.2} MB/s)",
        elapsed, mb_per_sec
    );

    // Verify all bytes match raw_data
    let t_storage = leecher_storage
        .torrents
        .read()
        .get(&info_hash)
        .unwrap()
        .clone();
    for i in 0..info.num_pieces() {
        let piece_obj = info.piece(i);
        let p_storage = t_storage.piece(piece_obj, info.piece_hash_v1(i));
        let mut check_buf = vec![0u8; piece_obj.length as usize];
        p_storage.read_at(&mut check_buf, 0).unwrap();
        let start = i as usize * PIECE_LEN;
        let end = (start + piece_obj.length as usize).min(TOTAL_SIZE);
        assert_eq!(&check_buf[..], &raw_data[start..end]);
    }

    seeder.close();
    leecher.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_reader_seeks_into_missing_data_and_gets_first_64_kib() {
    const TOTAL_SIZE: usize = 16 * 1024 * 1024; // 16 MiB
    const PIECE_LEN: usize = 512 * 1024; // 512 KiB

    let (info, info_hash, raw_data) = create_torrent_with_data(TOTAL_SIZE, PIECE_LEN);

    // Seeder
    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let seeder_cfg = ClientConfig {
        seed: true,
        listen_port: 0,
        default_storage: seeder_storage.clone(),
        ..Default::default()
    };
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_port = loop {
        let p = seeder.local_port();
        if p > 0 {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let seeder_addr = format!("127.0.0.1:{}", seeder_port).parse().unwrap();

    let (seeder_torrent, _) = seeder
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(info_hash),
            display_name: "seeder".to_string(),
            ..Default::default()
        })
        .unwrap();
    seeder_torrent
        .inner
        .set_info(Arc::new(info.clone()))
        .unwrap();
    {
        let t_storage = seeder_storage
            .torrents
            .read()
            .get(&info_hash)
            .unwrap()
            .clone();
        for i in 0..info.num_pieces() {
            let piece_obj = info.piece(i);
            let p_storage = t_storage.piece(piece_obj, info.piece_hash_v1(i));
            let start = i as usize * PIECE_LEN;
            let end = (start + piece_obj.length as usize).min(TOTAL_SIZE);
            p_storage.write_at(&raw_data[start..end], 0).unwrap();
            p_storage.mark_complete().unwrap();
            seeder_torrent.inner.pieces.write()[i as usize].complete = true;
            seeder_torrent.inner.pieces.write()[i as usize].ok = true;
        }
    }

    // Leecher
    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let leecher_cfg = ClientConfig {
        listen_port: 0,
        default_storage: leecher_storage.clone(),
        ..Default::default()
    };
    let leecher = Client::new(leecher_cfg).unwrap();

    let (leecher_torrent, _) = leecher
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(info_hash),
            display_name: "leecher".to_string(),
            ..Default::default()
        })
        .unwrap();
    leecher_torrent
        .inner
        .set_info(Arc::new(info.clone()))
        .unwrap();

    // Connect leecher to seeder
    leecher
        .inner
        .dial_peer(
            leecher_torrent.inner.clone(),
            seeder_addr,
            wt_types::PeerSource::Direct,
        )
        .await
        .unwrap();

    // Wait 50ms for handshake and unchoke exchange
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reader seeks to offset 8 MiB (in missing data) and reads 64 KiB
    let seek_offset: u64 = 8 * 1024 * 1024;
    let mut reader = leecher_torrent.new_reader();
    reader.set_responsive();
    reader.seek(SeekFrom::Start(seek_offset)).unwrap();

    let start_read = Instant::now();
    let read_task = tokio::task::spawn_blocking(move || {
        let mut buf = vec![0u8; 64 * 1024]; // 64 KiB
        let mut total_read = 0;
        while total_read < buf.len() {
            let n = reader.read(&mut buf[total_read..]).unwrap();
            total_read += n;
        }
        buf
    });

    let buf = tokio::time::timeout(Duration::from_secs(10), read_task)
        .await
        .expect("Seek read timed out")
        .expect("Join error");

    assert_eq!(buf.len(), 64 * 1024);
    let expected = &raw_data[seek_offset as usize..seek_offset as usize + 64 * 1024];
    assert_eq!(&buf[..], expected);
    println!("Seek read 64 KiB completed in {:.3?}", start_read.elapsed());

    seeder.close();
    leecher.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_magnet_ut_metadata_transfer() {
    const TOTAL_SIZE: usize = 2 * 1024 * 1024; // 2 MiB
    const PIECE_LEN: usize = 256 * 1024;

    let (info, info_hash, raw_data) = create_torrent_with_data(TOTAL_SIZE, PIECE_LEN);
    let bencoded_info = wt_meta::encode_info(&info).unwrap();

    // Seeder has metainfo and data
    let seeder_storage = Arc::new(MemoryClientStorage::new());
    let seeder_cfg = ClientConfig {
        seed: true,
        listen_port: 0,
        default_storage: seeder_storage.clone(),
        ..Default::default()
    };
    let seeder = Client::new(seeder_cfg).unwrap();
    let seeder_port = loop {
        let p = seeder.local_port();
        if p > 0 {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let seeder_addr = format!("127.0.0.1:{}", seeder_port).parse().unwrap();

    let (seeder_torrent, _) = seeder
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(info_hash),
            info_bytes: Some(bencoded_info.clone()),
            display_name: "seeder_magnet".to_string(),
            ..Default::default()
        })
        .unwrap();
    seeder_torrent
        .inner
        .metainfo_raw
        .write()
        .replace(wt_types::MetaInfo {
            info_bytes: bencoded_info.clone(),
            ..Default::default()
        });

    // Fill seeder data
    {
        let t_storage = seeder_storage
            .torrents
            .read()
            .get(&info_hash)
            .unwrap()
            .clone();
        for i in 0..info.num_pieces() {
            let piece_obj = info.piece(i);
            let p_storage = t_storage.piece(piece_obj, info.piece_hash_v1(i));
            let start = i as usize * PIECE_LEN;
            let end = (start + piece_obj.length as usize).min(TOTAL_SIZE);
            p_storage.write_at(&raw_data[start..end], 0).unwrap();
            p_storage.mark_complete().unwrap();
            seeder_torrent.inner.pieces.write()[i as usize].complete = true;
        }
    }

    // Leecher starts with magnet ONLY (no info_bytes)
    let leecher_storage = Arc::new(MemoryClientStorage::new());
    let leecher_cfg = ClientConfig {
        listen_port: 0,
        default_storage: leecher_storage.clone(),
        ..Default::default()
    };
    let leecher = Client::new(leecher_cfg).unwrap();

    let (leecher_torrent, _) = leecher
        .add_torrent_spec(TorrentSpec {
            info_hash: Some(info_hash),
            display_name: "magnet_leecher".to_string(),
            ..Default::default()
        })
        .unwrap();

    // Verify leecher starts with no info
    assert!(leecher_torrent.info().is_none());

    // Connect leecher to seeder
    leecher
        .inner
        .dial_peer(
            leecher_torrent.inner.clone(),
            seeder_addr,
            wt_types::PeerSource::Direct,
        )
        .await
        .unwrap();

    // Leecher should fetch metadata via ut_metadata and populate info
    let wait_res = leecher_torrent.wait_info(Some(Duration::from_secs(10)));
    assert!(wait_res.is_ok());
    let fetched_info = wait_res.unwrap();
    assert_eq!(fetched_info.name, info.name);
    assert_eq!(fetched_info.total_length(), TOTAL_SIZE as i64);

    seeder.close();
    leecher.close();
}

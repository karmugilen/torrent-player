use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;
use wt_core::arrival::ArrivalModel;
use wt_core::torrent::TorrentInner;
use wt_core::{CancelToken, Info, InfoHash, MemoryClientStorage, TorrentStorage};

fn make_test_torrent(
    total_size: i64,
    piece_len: i64,
) -> (Arc<TorrentInner>, Arc<MemoryClientStorage>) {
    let num_pieces = (total_size + piece_len - 1) / piece_len;
    let mut pieces = Vec::new();
    for i in 0..num_pieces {
        let mut h = [0u8; 20];
        h[0] = (i + 1) as u8;
        pieces.extend_from_slice(&h);
    }
    let info = Info {
        name: "test_reader".to_string(),
        piece_length: piece_len,
        pieces,
        files: vec![wt_types::FileInfo {
            path: Vec::new(),
            length: total_size,
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
    let storage = Arc::new(MemoryClientStorage::new());
    let arrival_model = Arc::new(parking_lot::RwLock::new(ArrivalModel::new()));
    let tor = Arc::new(TorrentInner::new(
        InfoHash([4u8; 20]),
        None,
        "test_reader".to_string(),
        storage.clone(),
        arrival_model,
    ));
    tor.set_info(Arc::new(info)).unwrap();
    (tor, storage)
}

#[test]
fn test_reader_read_completed_data() {
    let (tor, storage) = make_test_torrent(1024 * 1024, 256 * 1024);
    let t = wt_core::Torrent { inner: tor.clone() };

    // Fill piece 0 with pattern 0x5a
    let info = tor.info.read().as_ref().unwrap().clone();
    let tor_storage = storage.torrents.read().get(&tor.info_hash).unwrap().clone();
    let p0 = tor_storage.piece(info.piece(0), info.piece_hash_v1(0));
    let buf_data = vec![0x5a; 256 * 1024];
    p0.write_at(&buf_data, 0).unwrap();
    tor.pieces_mut()[0].complete = true;

    let mut reader = t.new_reader();
    let mut read_buf = vec![0u8; 100];
    let n = reader.read(&mut read_buf).unwrap();
    assert_eq!(n, 100);
    assert!(read_buf.iter().all(|&b| b == 0x5a));
}

#[test]
fn test_reader_seek_and_read() {
    let (tor, storage) = make_test_torrent(1024 * 1024, 256 * 1024);
    let t = wt_core::Torrent { inner: tor.clone() };

    let info = tor.info.read().as_ref().unwrap().clone();
    let tor_storage = storage.torrents.read().get(&tor.info_hash).unwrap().clone();
    let p1 = tor_storage.piece(info.piece(1), info.piece_hash_v1(1));
    let mut p1_data = vec![0x00; 256 * 1024];
    p1_data[100..200].copy_from_slice(&[0x42; 100]);
    p1.write_at(&p1_data, 0).unwrap();
    tor.pieces_mut()[1].complete = true;

    let mut reader = t.new_reader();
    let seek_pos = 256 * 1024 + 100;
    reader.seek(SeekFrom::Start(seek_pos as u64)).unwrap();

    let mut read_buf = vec![0u8; 100];
    let n = reader.read(&mut read_buf).unwrap();
    assert_eq!(n, 100);
    assert_eq!(read_buf, vec![0x42; 100]);
}

#[test]
fn test_reader_cancel_interrupted() {
    let (tor, _) = make_test_torrent(1024 * 1024, 256 * 1024);
    let t = wt_core::Torrent { inner: tor };
    let mut reader = t.new_reader();
    let cancel = CancelToken::default();
    cancel.cancel(); // cancel immediately
    reader.set_cancel(cancel);

    let mut buf = vec![0u8; 1024];
    let res = reader.read_timeout(&mut buf, Some(Duration::from_millis(50)));
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
}

#[test]
fn test_reader_responsive_mode() {
    let (tor, storage) = make_test_torrent(1024 * 1024, 256 * 1024);
    let t = wt_core::Torrent { inner: tor.clone() };

    // In responsive mode, write only 1 block (16 KiB) of piece 0 without marking piece complete
    let info = tor.info.read().as_ref().unwrap().clone();
    let tor_storage = storage.torrents.read().get(&tor.info_hash).unwrap().clone();
    let p0 = tor_storage.piece(info.piece(0), info.piece_hash_v1(0));
    let chunk_data = vec![0x77; 16 * 1024];
    p0.write_at(&chunk_data, 0).unwrap();
    tor.dirty_chunks.insert(0); // mark block 0 dirty

    let mut reader = t.new_reader();
    reader.set_responsive();

    let mut buf = vec![0u8; 1024];
    let n = reader
        .read_timeout(&mut buf, Some(Duration::from_millis(100)))
        .unwrap();
    assert_eq!(n, 1024);
    assert_eq!(buf, vec![0x77; 1024]);
}

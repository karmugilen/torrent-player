//! Shared helpers for wt-core end-to-end transfer tests.
//!
//! Fixtures live under `/home/kar/.cache/wt-tmp/wt-core-e2e` (≤256 MiB) or in RAM.
//! Re-exports / wraps `wt_core::MemoryClientStorage` as the in-memory ClientStorage
//! double used by every e2e case.

#![allow(dead_code)]

use sha1::{Digest, Sha1};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use wt_core::{
    Client, ClientConfig, ClientStorage, FileInfo, Info, InfoHash, MemoryClientStorage, PeerInfo,
    PeerSource, PexFlags, Torrent, TorrentSpec, TorrentStorage,
};
use wt_meta::{encode_info, info_hash_v1};

pub const FIXTURE_ROOT: &str = "/home/kar/.cache/wt-tmp/wt-core-e2e";

/// Go `testutil.GreetingFileContents` — includes a NUL so blob/text confusion fails loudly.
pub const GREETING_CONTENTS: &[u8] = b"hello,\x00world\n";
pub const GREETING_NAME: &str = "greeting";
/// Go `Greeting.Generate(5)` piece length.
pub const GREETING_PIECE_LEN: i64 = 5;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique directory under the fixture root; caller should remove when done.
pub fn fixture_dir(prefix: &str) -> PathBuf {
    let _ = fs::create_dir_all(FIXTURE_ROOT);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(FIXTURE_ROOT).join(format!("{prefix}-{id}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

pub fn cleanup_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Deterministic payload with SHA-1 piece hashes and correct infohash from `encode_info`.
pub fn patterned_torrent(
    name: &str,
    length: usize,
    piece_length: usize,
) -> (Info, InfoHash, Vec<u8>, Vec<u8>) {
    let mut data = vec![0u8; length];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i.wrapping_mul(17) + 31) % 256) as u8;
    }
    let num_pieces = length.div_ceil(piece_length);
    let mut pieces = Vec::with_capacity(num_pieces * 20);
    for i in 0..num_pieces {
        let start = i * piece_length;
        let end = (start + piece_length).min(length);
        let mut h = Sha1::new();
        h.update(&data[start..end]);
        pieces.extend_from_slice(&h.finalize());
    }
    let info = Info {
        name: name.into(),
        piece_length: piece_length as i64,
        pieces,
        files: vec![FileInfo {
            path: Vec::new(),
            length: length as i64,
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
    let info_bytes = encode_info(&info).expect("encode_info");
    let ih = info_hash_v1(&info_bytes);
    (info, ih, data, info_bytes)
}

/// Multi-file torrent; `files` is `(path_component, length)`.
pub fn multi_file_torrent(
    name: &str,
    piece_length: usize,
    files: &[(&str, usize)],
) -> (Info, InfoHash, Vec<u8>, Vec<u8>) {
    let total: usize = files.iter().map(|(_, n)| *n).sum();
    let mut data = vec![0u8; total];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i.wrapping_mul(13) + 7) % 256) as u8;
    }
    let mut offset = 0i64;
    let mut file_infos = Vec::with_capacity(files.len());
    for &(path, len) in files {
        file_infos.push(FileInfo {
            path: vec![path.into()],
            length: len as i64,
            torrent_offset: offset,
            attr: String::new(),
            pieces_root: None,
        });
        offset += len as i64;
    }
    let num_pieces = total.div_ceil(piece_length);
    let mut pieces = Vec::with_capacity(num_pieces * 20);
    for i in 0..num_pieces {
        let start = i * piece_length;
        let end = (start + piece_length).min(total);
        let mut h = Sha1::new();
        h.update(&data[start..end]);
        pieces.extend_from_slice(&h.finalize());
    }
    let info = Info {
        name: name.into(),
        piece_length: piece_length as i64,
        pieces,
        files: file_infos,
        single_file: false,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    };
    let info_bytes = encode_info(&info).expect("encode_info");
    let ih = info_hash_v1(&info_bytes);
    (info, ih, data, info_bytes)
}

/// Go greeting torrent (13 bytes, piece length 5 → 3 pieces).
pub fn greeting_torrent() -> (Info, InfoHash, Vec<u8>, Vec<u8>) {
    let length = GREETING_CONTENTS.len();
    let piece_length = GREETING_PIECE_LEN as usize;
    let data = GREETING_CONTENTS.to_vec();
    let num_pieces = length.div_ceil(piece_length);
    let mut pieces = Vec::with_capacity(num_pieces * 20);
    for i in 0..num_pieces {
        let start = i * piece_length;
        let end = (start + piece_length).min(length);
        let mut h = Sha1::new();
        h.update(&data[start..end]);
        pieces.extend_from_slice(&h.finalize());
    }
    let info = Info {
        name: GREETING_NAME.into(),
        piece_length: GREETING_PIECE_LEN,
        pieces,
        files: vec![FileInfo {
            path: Vec::new(),
            length: length as i64,
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
    let info_bytes = encode_info(&info).expect("encode_info");
    let ih = info_hash_v1(&info_bytes);
    (info, ih, data, info_bytes)
}

pub fn testing_config(storage: Arc<MemoryClientStorage>, seed: bool) -> ClientConfig {
    ClientConfig {
        listen_port: 0,
        no_dht: true,
        disable_webtorrent: true,
        seed,
        dial_for_peer_conns: true,
        accept_peer_connections: true,
        default_storage: storage,
        periodically_announce_torrents_to_dht: false,
        ..Default::default()
    }
}

pub async fn wait_local_port(client: &Client) -> u16 {
    for _ in 0..500 {
        let p = client.local_port();
        if p > 0 {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("client listen port not bound");
}

pub fn peer_info(addr: SocketAddr) -> PeerInfo {
    PeerInfo {
        id: None,
        addr,
        source: PeerSource::Direct,
        supports_encryption: false,
        pex: PexFlags {
            prefers_encryption: false,
            seed_only: false,
            supports_utp: true,
            supports_holepunch: false,
            reachable: true,
        },
        trusted: true,
    }
}

/// Seed every piece into MemoryClientStorage and mark torrent piece state complete.
pub fn seed_memory(
    storage: &MemoryClientStorage,
    torrent: &Torrent,
    info: &Info,
    info_hash: InfoHash,
    data: &[u8],
) {
    // Ensure storage opened for this hash (add_torrent_spec usually does).
    let _ = storage.open_torrent(info, info_hash);
    let t_storage = storage
        .torrents
        .read()
        .get(&info_hash)
        .expect("torrent storage missing")
        .clone();
    let piece_len = info.piece_length as usize;
    for i in 0..info.num_pieces() {
        let piece_obj = info.piece(i);
        let p_storage = t_storage.piece(piece_obj, info.piece_hash_v1(i));
        let start = i as usize * piece_len;
        let end = (start + piece_obj.length as usize).min(data.len());
        p_storage.write_at(&data[start..end], 0).unwrap();
        p_storage.mark_complete().unwrap();
        if let Some(slot) = torrent.inner.pieces.write().get_mut(i as usize) {
            slot.complete = true;
            slot.ok = true;
        }
    }
}

pub fn torrent_spec(
    info_hash: InfoHash,
    info_bytes: Option<Vec<u8>>,
    name: &str,
    storage: Option<Arc<MemoryClientStorage>>,
) -> TorrentSpec {
    TorrentSpec {
        info_hash: Some(info_hash),
        info_bytes,
        display_name: name.into(),
        storage: storage.map(|s| s as Arc<dyn wt_types::ClientStorage>),
        ..Default::default()
    }
}

/// Dial seeder from leecher (Go `AddClientPeer` analogue via public ClientInner).
pub async fn add_client_peer(
    _leecher: &Client,
    leecher_torrent: &Torrent,
    seeder_addr: SocketAddr,
) {
    leecher_torrent.add_peers(vec![peer_info(seeder_addr)]);
}

pub async fn wait_bytes_completed(torrent: &Torrent, want: i64, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if torrent.bytes_completed() >= want {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timeout waiting for bytes_completed>={want}, have {}",
                torrent.bytes_completed()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn read_all_from_storage(
    storage: &MemoryClientStorage,
    info: &Info,
    info_hash: InfoHash,
) -> Vec<u8> {
    let t_storage = storage.torrents.read().get(&info_hash).unwrap().clone();
    let mut out = Vec::with_capacity(info.total_length() as usize);
    let piece_len = info.piece_length as usize;
    for i in 0..info.num_pieces() {
        let piece_obj = info.piece(i);
        let p_storage = t_storage.piece(piece_obj, info.piece_hash_v1(i));
        let mut buf = vec![0u8; piece_obj.length as usize];
        p_storage.read_at(&mut buf, 0).unwrap();
        out.extend_from_slice(&buf);
        let _ = piece_len;
    }
    out
}

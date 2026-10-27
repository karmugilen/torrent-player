//! Shared helpers for wt-storage Go→Rust port integration tests.
//! Fixtures live under `/home/kar/.cache/wt-tmp/wt-storage-tests` (≤256 MiB total)
//! and are deleted when the TempDir drops.

#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::net::SocketAddr;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use tempfile::TempDir;
use wt_types::{FileInfo, Info, InfoHash, PeerInfo, PeerSource, PexFlags, Piece};

pub const FIXTURE_ROOT: &str = "/home/kar/.cache/wt-tmp/wt-storage-tests";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a unique temp directory under the fixture root. Drop deletes it.
pub fn test_temp_dir(prefix: &str) -> TempDir {
    fs::create_dir_all(FIXTURE_ROOT).expect("create fixture root");
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    tempfile::Builder::new()
        .prefix(&format!("{prefix}-{id}-"))
        .tempdir_in(FIXTURE_ROOT)
        .expect("create tempdir under fixture root")
}

pub fn open_rw(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .expect("open rw file")
}

pub fn open_rw_no_trunc(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .expect("open rw file")
}

/// Single-file Info with `length` bytes and `piece_length`, zero piece hashes.
pub fn single_file_info(name: &str, length: i64, piece_length: i64) -> Info {
    let num_pieces = if piece_length <= 0 || length <= 0 {
        0
    } else {
        ((length + piece_length - 1) / piece_length) as usize
    };
    Info {
        name: name.into(),
        piece_length,
        pieces: vec![0u8; num_pieces * 20],
        files: vec![FileInfo {
            length,
            path: vec![],
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    }
}

/// Multi-file Info; `files` is `(path_component, length)`.
pub fn multi_file_info(name: &str, piece_length: i64, files: &[(&str, i64)]) -> Info {
    let mut offset = 0i64;
    let mut out = Vec::with_capacity(files.len());
    for &(path, length) in files {
        out.push(FileInfo {
            length,
            path: vec![path.into()],
            torrent_offset: offset,
            attr: String::new(),
            pieces_root: None,
        });
        offset += length;
    }
    let total = offset;
    let num_pieces = if piece_length <= 0 || total <= 0 {
        0
    } else {
        ((total + piece_length - 1) / piece_length) as usize
    };
    Info {
        name: name.into(),
        piece_length,
        pieces: vec![0u8; num_pieces * 20],
        files: out,
        single_file: false,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    }
}

pub fn info_hash_zero() -> InfoHash {
    InfoHash([0u8; 20])
}

pub fn info_hash_bytes(b: [u8; 20]) -> InfoHash {
    InfoHash(b)
}

pub fn piece_of(info: &Info, index: u32) -> Piece {
    info.piece(index)
}

/// Keep `files` alive while returning RawFd slots for attach_descriptors.
pub fn fds_from_files(files: &[Option<&File>]) -> Vec<Option<RawFd>> {
    files
        .iter()
        .map(|f| f.map(|file| file.as_raw_fd()))
        .collect()
}

pub fn peer_info_tcp(addr: &str) -> PeerInfo {
    let addr: SocketAddr = addr.parse().expect("peer addr");
    PeerInfo {
        id: None,
        addr,
        source: PeerSource::Tracker,
        supports_encryption: false,
        pex: PexFlags {
            prefers_encryption: false,
            seed_only: false,
            supports_utp: false,
            supports_holepunch: false,
            reachable: false,
        },
        trusted: false,
    }
}

pub fn path_bytes(path: &Path) -> i64 {
    fn walk(p: &Path) -> i64 {
        let meta = match fs::metadata(p) {
            Ok(m) => m,
            Err(_) => return 0,
        };
        if meta.is_file() {
            return meta.len() as i64;
        }
        if !meta.is_dir() {
            return 0;
        }
        let mut sum = 0i64;
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                sum += walk(&e.path());
            }
        }
        sum
    }
    walk(path)
}

pub fn ensure_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
}

pub fn write_bytes(path: &Path, data: &[u8]) {
    ensure_parent(path);
    fs::write(path, data).expect("write bytes");
}

/// Patterned payload used by Go directStorageFixture.
pub fn patterned_payload(piece_length: usize, pieces: usize) -> Vec<u8> {
    let mut data = vec![0u8; piece_length * pieces];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i * 17 + i / piece_length) % 256) as u8;
    }
    data
}

/// Info with real SHA-1 piece hashes for `data`.
pub fn info_with_hashes(name: &str, piece_length: i64, data: &[u8]) -> Info {
    use sha1::{Digest, Sha1};
    let mut pieces = Vec::new();
    let pl = piece_length as usize;
    for chunk in data.chunks(pl) {
        let mut h = Sha1::new();
        h.update(chunk);
        pieces.extend_from_slice(&h.finalize());
    }
    Info {
        name: name.into(),
        piece_length,
        pieces,
        files: vec![FileInfo {
            length: data.len() as i64,
            path: vec![],
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    }
}

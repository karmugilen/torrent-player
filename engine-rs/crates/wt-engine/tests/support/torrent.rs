//! Deterministic torrent builders (payload + metainfo) for integration tests.

use sha1::{Digest, Sha1};
use wt_meta::{encode_info, encode_metainfo, info_hash_v1};
use wt_types::{FileInfo, Info, MetaInfo};

/// Repeating 8-byte motif used by Go fixtures (`abcdefgh`).
pub fn piece_payload(total: usize) -> Vec<u8> {
    let motif = b"abcdefgh";
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        let n = (total - out.len()).min(motif.len());
        out.extend_from_slice(&motif[..n]);
    }
    out
}

fn piece_hashes(data: &[u8], piece_length: usize) -> Vec<u8> {
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let end = (start + piece_length).min(data.len());
        let digest = Sha1::digest(&data[start..end]);
        pieces.extend_from_slice(&digest);
        start = end;
    }
    pieces
}

/// Single-file torrent named like Go's `movie.mp4` fixtures.
pub fn build_single_file_torrent(
    name: &str,
    piece_length: i64,
    piece_count: usize,
) -> (Vec<u8>, Vec<u8>, String) {
    let total = piece_length as usize * piece_count;
    let data = piece_payload(total);
    let pieces = piece_hashes(&data, piece_length as usize);
    let info = Info {
        name: name.into(),
        piece_length,
        pieces,
        files: vec![FileInfo {
            path: vec![],
            length: total as i64,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        ..Default::default()
    };
    let info_bytes = encode_info(&info).expect("encode_info");
    let hash = info_hash_v1(&info_bytes);
    let mi = MetaInfo {
        info_bytes,
        ..Default::default()
    };
    let torrent = encode_metainfo(&mi).expect("encode_metainfo");
    (torrent, data, hash.to_hex())
}

/// Multi-file torrent with explicit lengths (episode pack style).
pub fn build_multi_file_torrent(
    name: &str,
    piece_length: i64,
    files: &[(&str, i64)],
) -> (Vec<u8>, Vec<u8>, String) {
    let total: i64 = files.iter().map(|(_, l)| *l).sum();
    let data = piece_payload(total as usize);
    let pieces = piece_hashes(&data, piece_length as usize);
    let mut offset = 0i64;
    let file_infos: Vec<FileInfo> = files
        .iter()
        .map(|(path, length)| {
            let fi = FileInfo {
                path: path.split('/').map(|s| s.to_string()).collect(),
                length: *length,
                torrent_offset: offset,
                attr: String::new(),
                pieces_root: None,
            };
            offset += *length;
            fi
        })
        .collect();
    let info = Info {
        name: name.into(),
        piece_length,
        pieces,
        files: file_infos,
        ..Default::default()
    };
    let info_bytes = encode_info(&info).expect("encode_info");
    let hash = info_hash_v1(&info_bytes);
    let mi = MetaInfo {
        info_bytes,
        ..Default::default()
    };
    let torrent = encode_metainfo(&mi).expect("encode_metainfo");
    (torrent, data, hash.to_hex())
}

pub fn info_hash_hex(torrent_bytes: &[u8]) -> String {
    let (mi, _) = wt_meta::parse_torrent(torrent_bytes).expect("parse_torrent");
    info_hash_v1(&mi.info_bytes).to_hex()
}

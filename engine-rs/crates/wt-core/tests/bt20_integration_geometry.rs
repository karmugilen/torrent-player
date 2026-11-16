//! BEP 52 piece geometry. Every v2 file starts on a piece boundary and a
//! piece never spans files, so the piece holding a file's tail is short and
//! the payload has gaps: `Info::total_length()` (sum of file lengths) is not
//! the end of piece space. Hybrids list the gaps as pad files, whose bytes
//! are no piece's data (v1 hashing appends them as zeros).

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use wt_core::peer_conn::PeerTorrent;
use wt_core::picker::{block_coord_from_req_index, piece_req_index_range, Geometry};
use wt_core::{Client, ClientConfig, TorrentSpec, BLOCK_SIZE};

const PURE_V2: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-test.torrent");
const HYBRID: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");

fn info_of(file: &[u8]) -> wt_core::Info {
    wt_meta::parse_info(&wt_meta::parse_metainfo(file).unwrap().info_bytes).unwrap()
}

fn fixtures() -> [(&'static str, &'static [u8]); 2] {
    [("pure v2", PURE_V2), ("hybrid", HYBRID)]
}

/// Pieces whose block count under `geom` is not `ceil(piece_data_length/16 KiB)`.
fn mis_sized_pieces(info: &wt_core::Info, geom: &Geometry) -> Vec<u32> {
    let block = i64::from(BLOCK_SIZE);
    (0..info.num_pieces())
        .filter(|&p| {
            let (b, e) = piece_req_index_range(p, info.piece_length, geom);
            let want = (info.piece_data_length(p) + block - 1) / block;
            i64::from(e - b) != want
        })
        .collect()
}

#[test]
fn pure_v2_fixture_has_unaligned_files() {
    let info = info_of(PURE_V2);
    assert!(info.has_v2() && !info.has_v1());
    assert!(info.payload_end() > info.total_length(), "v2 files are piece aligned");
    // The old total-length geometry got 16 of 371 pieces wrong, the last 6
    // with no blocks at all.
    let old = Geometry::v1(info.piece_length, info.total_length());
    assert_eq!(mis_sized_pieces(&info, &old).len(), 16);
    let hy = info_of(HYBRID);
    let old = Geometry::v1(hy.piece_length, hy.total_length());
    assert_eq!(mis_sized_pieces(&hy, &old).len(), 15, "hybrid too");
}

#[test]
fn pure_v2_block_geometry_matches_piece_data_length() {
    let info = info_of(PURE_V2);
    let geom = Geometry::of(&info);
    let bad = mis_sized_pieces(&info, &geom);
    assert!(bad.is_empty(), "{} of {} pieces mis-sized: {:?}", bad.len(), info.num_pieces(), bad);
}

/// Every piece of both fixtures: right block count, last block ends exactly
/// at the piece's data end, and nothing is requested past it.
#[test]
fn every_piece_of_both_fixtures_has_the_right_blocks() {
    for (name, file) in fixtures() {
        let info = info_of(file);
        let geom = Geometry::of(&info);
        assert!(geom.is_v2(), "{name}");
        assert_eq!(geom.end(), info.payload_end(), "{name}");
        assert!(mis_sized_pieces(&info, &geom).is_empty(), "{name}");
        for p in 0..info.num_pieces() {
            let data = info.piece_data_length(p);
            assert_eq!(geom.piece_data_len(p), data, "{name} piece {p}");
            let (b, e) = piece_req_index_range(p, info.piece_length, &geom);
            let mut covered = 0i64;
            for req in b..e {
                let c = block_coord_from_req_index(req, info.piece_length, &geom);
                assert_eq!(c.piece, p);
                assert!(c.length > 0 && i64::from(c.offset + c.length) <= data, "{name} piece {p} req {req}");
                covered += i64::from(c.length);
            }
            assert_eq!(covered, data, "{name} piece {p}: blocks cover the data exactly");
        }
    }
}

/// A reader of the last file can read its last byte (it lies past
/// `total_length()` in pure v2 torrent space), and the torrent's piece
/// entries and completion bookkeeping use the same geometry.
#[test]
fn reader_reads_last_byte_of_last_file() {
    for (name, file) in fixtures() {
        let client = Client::new(ClientConfig {
            no_dht: true,
            local_service_discovery: false,
            ..Default::default()
        })
        .unwrap();
        let spec = TorrentSpec {
            info_bytes: Some(wt_meta::parse_metainfo(file).unwrap().info_bytes),
            ..Default::default()
        };
        let (t, _) = client.add_torrent_spec(spec).unwrap();
        let inner = t.inner.clone();
        let info = inner.info.read().clone().unwrap();
        let last = t.files().into_iter().filter(|f| f.length() > 0).last().unwrap();
        let last_byte = last.offset() + last.length() - 1;
        let piece = (last_byte / info.piece_length) as u32;
        assert_eq!(piece, info.num_pieces() - 1, "{name}: last byte is in the last piece");
        assert_eq!(
            inner.pieces.read()[piece as usize].chunks_total as i64,
            (info.piece_data_length(piece) + i64::from(BLOCK_SIZE) - 1) / i64::from(BLOCK_SIZE),
            "{name}: piece entry agrees"
        );

        // Deliver the whole last piece, block by block, as from a peer.
        let data_len = info.piece_data_length(piece);
        let pattern = |off: i64| (off % 251) as u8;
        let (b, e) = piece_req_index_range(piece, info.piece_length, &inner.geometry());
        assert!(e > b, "{name}: last piece has blocks");
        for req in b..e {
            let c = block_coord_from_req_index(req, info.piece_length, &inner.geometry());
            let bytes: Vec<u8> = (0..c.length as i64).map(|i| pattern(i64::from(c.offset) + i)).collect();
            inner.write_chunk(piece, c.offset, &bytes).unwrap();
            inner.mark_chunk_dirty(req);
        }
        assert!(inner.piece_all_dirty(piece), "{name}: all blocks of the piece are dirty");

        let mut r = last.new_reader();
        r.set_responsive();
        r.seek(SeekFrom::End(-1)).unwrap();
        let mut one = [0u8; 1];
        let n = r.read_timeout(&mut one, Some(Duration::from_secs(5))).unwrap();
        assert_eq!(n, 1, "{name}");
        assert_eq!(one[0], pattern(data_len - 1), "{name}: last byte content");
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "{name}: EOF after the last byte");
        client.close();
    }
}

/// Hybrid: a piece followed by a pad file is complete once its data blocks
/// are in. Pad blocks were part of its range before, but neither peers
/// (writes past the data are refused) nor web seeds (extents stop at the
/// data) could ever fill them, so such pieces never got hashed.
#[test]
fn hybrid_piece_before_pad_completes_without_pad_blocks() {
    let client = Client::new(ClientConfig { no_dht: true, local_service_discovery: false, ..Default::default() }).unwrap();
    let spec = TorrentSpec {
        info_bytes: Some(wt_meta::parse_metainfo(HYBRID).unwrap().info_bytes),
        ..Default::default()
    };
    let (t, _) = client.add_torrent_spec(spec).unwrap();
    let inner = t.inner.clone();
    let info = inner.info.read().clone().unwrap();
    // wt-meta lays hybrid files out from the v2 file tree (aligned offsets,
    // pads implicit), so the gap is there with or without pad entries.
    let piece = (0..info.num_pieces() - 1)
        .find(|&p| info.piece_data_length(p) < info.piece_length)
        .expect("a file-tail piece followed by padding");
    let geom = inner.geometry();
    let (b, e) = piece_req_index_range(piece, info.piece_length, &geom);
    for req in b..e {
        let c = block_coord_from_req_index(req, info.piece_length, &geom);
        inner.write_chunk(piece, c.offset, &vec![1u8; c.length as usize]).expect("data block write");
        inner.mark_chunk_dirty(req);
    }
    assert!(inner.piece_all_dirty(piece));
    client.close();
}

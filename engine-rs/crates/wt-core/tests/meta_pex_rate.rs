//! Integration smoke for metadata / pex / ratelimit public APIs.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use wt_core::metadata::{MetadataExchange, PeerMetadataState, METADATA_PIECE_SIZE};
use wt_core::pex::{PexState, PEX_MAX_DELTA};
use wt_core::ratelimit::{Limiter, DEFAULT_DOWNLOAD_BURST, INF};
use wt_meta::{info_hash_v1, parse_metainfo};
use wt_types::PexFlags;

#[test]
fn rate_inf_and_burst() {
    let l = Limiter::infinite();
    assert!(l.limit().is_infinite());
    l.ensure_default_burst(DEFAULT_DOWNLOAD_BURST);
    assert_eq!(l.burst(), DEFAULT_DOWNLOAD_BURST);
    assert!(l.allow_n(1024));
    l.set_limit(2048.0);
    assert!((l.limit() - 2048.0).abs() < f64::EPSILON);
    let _ = INF;
}

#[test]
fn pex_delta_cap_and_peers() {
    let mut s = PexState::new();
    for i in 0..(PEX_MAX_DELTA + 10) {
        s.add(
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(10, 0, 0, 1),
                1000 + i as u16,
            )),
            PexFlags::default(),
        );
    }
    let (full, _) = s.genmsg(None);
    assert_eq!(full.added.len(), PEX_MAX_DELTA + 10);

    let (delta, _) = s.genmsg(Some(wt_core::pex::PexCursor::start()));
    assert!(delta.added.len() <= PEX_MAX_DELTA);

    let mut msg = wt_wire::ext::PexMsg::new();
    msg.added
        .push(SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 6881));
    let mut s = PexState::new();
    let mut last = None;
    let peers = s.ingest(&msg, Instant::now(), &mut last).unwrap();
    assert_eq!(peers[0].source, wt_types::PeerSource::Pex);
    let _ = Duration::from_secs(120);
}

#[test]
fn metadata_multi_peer_assembly() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wt-meta/testdata/sintel.torrent"
    );
    let mi = parse_metainfo(&std::fs::read(path).unwrap()).unwrap();
    let ih = info_hash_v1(&mi.info_bytes);
    assert!(mi.info_bytes.len() > METADATA_PIECE_SIZE);

    let mut meta = MetadataExchange::new(Some(ih), None);
    meta.set_size(mi.info_bytes.len() as u32).unwrap();
    let n = meta.piece_count();
    let mut peer_states = vec![PeerMetadataState::new(); 3];
    let mut verified = None;
    for i in 0..n {
        let start = i * METADATA_PIECE_SIZE;
        let len = wt_core::metadata::MetadataExchange::piece_size(mi.info_bytes.len(), i as u32);
        let data = &mi.info_bytes[start..start + len];
        let p = &mut peer_states[i % 3];
        p.mark_requested(i as u32);
        if let Some(v) = meta.on_data(i as u32, data).unwrap() {
            verified = Some(v);
        }
        p.clear_requested(i as u32);
    }
    assert_eq!(verified.unwrap().info_hash_v1, ih);
}

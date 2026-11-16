//! BEP 52 at the client: a v2 hash learned after creation (btih magnet of a
//! hybrid, or merged in later) joins the v2 swarm, and inbound handshakes
//! offer the v2 upgrade bit exactly for torrents with a v2 hash.

use std::time::Duration;

use bytes::Bytes;
use wt_core::peer_conn::PeerTorrent;
use wt_core::{Client, ClientConfig, InfoHash, TorrentSpec};
use wt_wire::handshake::{default_peer_extension_bytes, handshake};

const HYBRID: &[u8] = include_bytes!("../../wt-meta/testdata/bittorrent-v2-hybrid-test.torrent");

fn client() -> Client {
    Client::new(ClientConfig {
        no_dht: true,
        local_service_discovery: false,
        ..Default::default()
    })
    .unwrap()
}

fn hybrid_info_bytes() -> Vec<u8> {
    wt_meta::parse_metainfo(HYBRID).unwrap().info_bytes
}

fn v1_only_info_bytes() -> Vec<u8> {
    let info = wt_core::Info {
        name: "v1".into(),
        piece_length: 16 * 1024,
        pieces: vec![0; 20],
        files: vec![wt_core::FileInfo { length: 1000, ..Default::default() }],
        single_file: true,
        ..Default::default()
    };
    wt_meta::encode_info(&info).unwrap()
}

/// Our side of an inbound handshake for `ih`: did the client's reply carry
/// the v2 upgrade bit?
fn inbound_reply_offers_v2(c: &Client, ih: InfoHash) -> bool {
    let port = c.local_port();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let hs = tokio::time::timeout(
            Duration::from_secs(5),
            handshake(&mut s, &ih, &[3u8; 20], &default_peer_extension_bytes()),
        )
        .await
        .expect("handshake timeout")
        .expect("handshake");
        hs.extensions.supports_v2_upgrade()
    })
}

fn feed_metadata(t: &wt_core::TorrentInner, bytes: &[u8]) -> bool {
    let mut done = false;
    for (i, chunk) in bytes.chunks(16 * 1024).enumerate() {
        done = t
            .on_metadata_data(i as u32, bytes.len() as u32, Bytes::copy_from_slice(chunk))
            .unwrap();
    }
    done
}

#[test]
fn inbound_handshake_offers_v2_upgrade_only_for_v2_torrents() {
    let c = client();
    assert_ne!(c.local_port(), 0);
    let hy = hybrid_info_bytes();
    let v1o = v1_only_info_bytes();
    for ib in [&hy, &v1o] {
        c.add_torrent_spec(TorrentSpec { info_bytes: Some(ib.clone()), ..Default::default() }).unwrap();
    }
    let (hv1, hv2) = (wt_meta::info_hash_v1(&hy), wt_meta::info_hash_v2(&hy));
    assert!(inbound_reply_offers_v2(&c, hv1), "hybrid reached via v1 hash");
    assert!(inbound_reply_offers_v2(&c, hv2.to_short()), "hybrid reached via v2 hash");
    assert!(!inbound_reply_offers_v2(&c, wt_meta::info_hash_v1(&v1o)), "v1-only torrent");
    c.close();
}

/// btih magnet of a hybrid: the v2 hash is learned from the metadata, the
/// torrent joins the v2 swarm (short-hash alias) and inbound handshakes start
/// offering the upgrade.
#[test]
fn btih_magnet_of_hybrid_learns_v2_hash_from_metadata() {
    let c = client();
    let hy = hybrid_info_bytes();
    let (hv1, hv2) = (wt_meta::info_hash_v1(&hy), wt_meta::info_hash_v2(&hy));
    let (t, _) = c.add_torrent_spec(TorrentSpec { info_hash: Some(hv1), ..Default::default() }).unwrap();
    assert_eq!(t.info_hash_v2(), None);
    assert!(!c.inner.torrents_by_short.read().contains_key(&hv2.to_short()));
    assert!(!inbound_reply_offers_v2(&c, hv1), "nothing known about v2 yet");

    assert!(feed_metadata(&t.inner, &hy));
    assert_eq!(t.info_hash_v2(), Some(hv2));
    let aliased = c.inner.torrents_by_short.read().get(&hv2.to_short()).cloned();
    assert!(aliased.is_some_and(|a| std::sync::Arc::ptr_eq(&a, &t.inner)));
    assert!(t.inner.is_v2_handshake_hash(&hv2.to_short().0));
    assert!(inbound_reply_offers_v2(&c, hv1));
    c.close();
}

/// A btmh added over a running btih magnet sets the v2 hash (merge path).
#[test]
fn merging_btmh_into_btih_magnet_sets_v2_hash() {
    let c = client();
    let hy = hybrid_info_bytes();
    let (hv1, hv2) = (wt_meta::info_hash_v1(&hy), wt_meta::info_hash_v2(&hy));
    let (t, _) = c.add_torrent_spec(TorrentSpec { info_hash: Some(hv1), ..Default::default() }).unwrap();
    let (again, new) = c
        .add_torrent_spec(TorrentSpec { info_hash: Some(hv1), info_hash_v2: Some(hv2), ..Default::default() })
        .unwrap();
    assert!(!new && std::sync::Arc::ptr_eq(&again.inner, &t.inner));
    assert_eq!(t.info_hash_v2(), Some(hv2));
    assert!(c.inner.torrents_by_short.read().contains_key(&hv2.to_short()));
    // A conflicting second v2 hash is not taken.
    let other = wt_core::InfoHashV2([9u8; 32]);
    let _ = c.add_torrent_spec(TorrentSpec { info_hash: Some(hv1), info_hash_v2: Some(other), ..Default::default() });
    assert_eq!(t.info_hash_v2(), Some(hv2));
    c.close();
}

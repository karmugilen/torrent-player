use wt_core::{Client, ClientConfig, InfoHash, InfoHashV2, TorrentSpec};

#[test]
fn test_hybrid_magnet_trackers_register_both_short_infohashes() {
    let cfg = ClientConfig::default();
    let client = Client::new(cfg).unwrap();

    let v1 = InfoHash([0xab; 20]);
    let v2_bytes = [0xcd; 32];
    let v2 = InfoHashV2(v2_bytes);
    let v2_short = v2.to_short();

    let spec = TorrentSpec {
        info_hash: Some(v1),
        info_hash_v2: Some(v2),
        trackers: vec![vec![
            "http://127.0.0.1:1/announce".to_string(),
            "udp://127.0.0.1:1/announce".to_string(),
        ]],
        display_name: "hybrid_torrent".to_string(),
        ..Default::default()
    };

    let (tor, newly_added) = client.add_torrent_spec(spec).unwrap();
    assert!(newly_added);

    // Both short infohashes must be registered in the client!
    let short_map = client.inner.torrents_by_short.read();
    assert!(short_map.contains_key(&v1));
    assert!(short_map.contains_key(&v2_short));
    drop(short_map);

    // Dropping torrent drops it
    tor.drop_torrent();
    assert!(tor.closed());
}

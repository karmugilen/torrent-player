//! Parity tests against Go metainfo testdata.

use wt_meta::{
    encode_info, info_hash_v1, info_hash_v2, magnet_uri, parse_info, parse_magnet, parse_metainfo,
    validate_metainfo_piece_layers, Magnet,
};

fn load(name: &str) -> Vec<u8> {
    let path = format!("testdata/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// (filename, v1 hex, optional v2 hex, expected file count)
const TORRENTS: &[(&str, &str, Option<&str>, usize)] = &[
    (
        "23516C72685E8DB0C8F15553382A927F185C4F01.torrent",
        "23516c72685e8db0c8f15553382a927f185c4f01",
        None,
        4,
    ),
    (
        "SKODAOCTAVIA336x280_archive.torrent",
        "d4b197dff199aad447a9a352e31528adbbd97922",
        None,
        8,
    ),
    (
        "archlinux-2011.08.19-netinstall-i686.iso.torrent",
        "500f29c0c537f5e41c6af676b7633de9d080d237",
        None,
        1,
    ),
    (
        "bittorrent-v2-hybrid-test.torrent",
        "631a31dd0a46257d5078c0dee4e66e26f73e42ac",
        Some("d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb"),
        9,
    ),
    (
        "bittorrent-v2-test.torrent",
        "f987ab6bb50f831a861c3754ecd1b47dc2cf2e30",
        Some("caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e"),
        11,
    ),
    (
        "bootstrap.dat.torrent",
        "36719ba2cecf9f3bd7c5abfb7a88e939611b536c",
        None,
        1,
    ),
    (
        "continuum.torrent",
        "4029ef207642d5d6b8b9a0a484a103262f764710",
        None,
        4,
    ),
    (
        "flat-url-list.torrent",
        "9da24e606e4ed9c7b91c1772fb5bf98f82bd9687",
        None,
        8,
    ),
    (
        "issue_65a.torrent",
        "4fe050208d675e5cd967d41348a31ab11b316806",
        None,
        8,
    ),
    (
        "issue_65b.torrent",
        "d376200e98b64abdcfa257a63f6208a704e42d9f",
        None,
        8,
    ),
    (
        "sintel.torrent",
        "08ada5a7a6183aae1e09d831df6748d566095a10",
        None,
        11,
    ),
    (
        "trackerless.torrent",
        "1dc8b6dbbb81c58b71220e20908245f8f565433f",
        None,
        1,
    ),
];

#[test]
fn infohashes_match_go() {
    for &(name, v1_hex, v2_hex, nfiles) in TORRENTS {
        let data = load(name);
        let mi = parse_metainfo(&data).unwrap_or_else(|e| panic!("{name}: {e}"));
        let info = parse_info(&mi.info_bytes).unwrap_or_else(|e| panic!("{name} info: {e}"));
        let v1 = info_hash_v1(&mi.info_bytes);
        assert_eq!(v1.to_hex(), v1_hex, "{name} v1");
        assert_eq!(info.files.len(), nfiles, "{name} file count");
        match v2_hex {
            Some(h) => {
                assert!(info.has_v2(), "{name} should be v2");
                assert_eq!(info_hash_v2(&mi.info_bytes).to_hex(), h, "{name} v2");
                validate_metainfo_piece_layers(&mi, &info)
                    .unwrap_or_else(|e| panic!("{name} piece layers: {e}"));
            }
            None => {
                assert!(!info.has_v2() || info.meta_version != 2);
            }
        }
    }
}

#[test]
fn v1_info_bytes_roundtrip_exact() {
    // Go TestFile: marshal(info) == mi.InfoBytes for these torrents.
    for name in [
        "archlinux-2011.08.19-netinstall-i686.iso.torrent",
        "continuum.torrent",
        "23516C72685E8DB0C8F15553382A927F185C4F01.torrent",
        "trackerless.torrent",
    ] {
        let data = load(name);
        let mi = parse_metainfo(&data).unwrap();
        let info = parse_info(&mi.info_bytes).unwrap();
        let encoded = encode_info(&info).unwrap();
        assert_eq!(
            encoded, mi.info_bytes,
            "{name}: encode_info must match original info bytes"
        );
    }
}

#[test]
fn minimal_trailing_newline_fails() {
    let data = load("minimal-trailing-newline.torrent");
    assert!(parse_metainfo(&data).is_err());
}

#[test]
fn url_list_list_magnet() {
    let data = load("SKODAOCTAVIA336x280_archive.torrent");
    let mi = parse_metainfo(&data).unwrap();
    assert_eq!(mi.url_list.len(), 3);
    let m = Magnet {
        info_hash: Some(info_hash_v1(&mi.info_bytes)),
        trackers: distinct_trackers(&mi),
        web_seeds: mi.url_list.clone(),
        ..Magnet::default()
    };
    let expected = "magnet:?xt=urn:btih:d4b197dff199aad447a9a352e31528adbbd97922&tr=http%3A%2F%2Fbt1.archive.org%3A6969%2Fannounce&tr=http%3A%2F%2Fbt2.archive.org%3A6969%2Fannounce&ws=https%3A%2F%2Farchive.org%2Fdownload%2F&ws=http%3A%2F%2Fia601600.us.archive.org%2F26%2Fitems%2F&ws=http%3A%2F%2Fia801600.us.archive.org%2F26%2Fitems%2F";
    assert_eq!(magnet_uri(&m), expected);
}

#[test]
fn url_list_string_magnet() {
    let data = load("flat-url-list.torrent");
    let mi = parse_metainfo(&data).unwrap();
    assert_eq!(mi.url_list.len(), 1);
    let m = Magnet {
        info_hash: Some(info_hash_v1(&mi.info_bytes)),
        trackers: distinct_trackers(&mi),
        web_seeds: mi.url_list.clone(),
        ..Magnet::default()
    };
    let expected = "magnet:?xt=urn:btih:9da24e606e4ed9c7b91c1772fb5bf98f82bd9687&tr=http%3A%2F%2Fbt1.archive.org%3A6969%2Fannounce&tr=http%3A%2F%2Fbt2.archive.org%3A6969%2Fannounce&ws=https%3A%2F%2Farchive.org%2Fdownload%2F";
    assert_eq!(magnet_uri(&m), expected);
}

#[test]
fn bootstrap_magnetize() {
    let data = load("bootstrap.dat.torrent");
    let mi = parse_metainfo(&data).unwrap();
    let info = parse_info(&mi.info_bytes).unwrap();
    assert_eq!(info.name, "bootstrap.dat");
    let ih = info_hash_v1(&mi.info_bytes);
    assert_eq!(
        ih.0,
        [
            54, 113, 155, 162, 206, 207, 159, 59, 215, 197, 171, 251, 122, 136, 233, 57, 97, 27,
            83, 108,
        ]
    );
}

#[test]
fn bencode_torrent_roundtrip_as_value() {
    for name in [
        "archlinux-2011.08.19-netinstall-i686.iso.torrent",
        "continuum.torrent",
        "trackerless.torrent",
    ] {
        let data = load(name);
        let v = wt_meta::bencode::decode(&data).unwrap();
        let encoded = wt_meta::bencode::encode(&v);
        assert_eq!(encoded, data, "{name} bencode value roundtrip");
    }
}

#[test]
fn magnet_parse_roundtrip_example() {
    let uri = "magnet:?xt=urn:btih:51340689c960f0778a4387aef9b4b52fd08390cd&dn=Shit+Movie+%281985%29+1337p+-+Eru&tr=http%3A%2F%2Fhttp.was.great%21&tr=udp%3A%2F%2Fanti.piracy.honeypot%3A6969";
    let m = parse_magnet(uri).unwrap();
    let s = magnet_uri(&m);
    let m2 = parse_magnet(&s).unwrap();
    assert_eq!(m.info_hash, m2.info_hash);
    assert_eq!(m.display_name, m2.display_name);
    assert_eq!(m.trackers, m2.trackers);
}

fn distinct_trackers(mi: &wt_meta::MetaInfo) -> Vec<String> {
    let mut t = Vec::new();
    for tier in &mi.announce_list {
        for u in tier {
            if !t.contains(u) {
                t.push(u.clone());
            }
        }
    }
    if t.is_empty() && !mi.announce.is_empty() {
        t.push(mi.announce.clone());
    }
    t
}

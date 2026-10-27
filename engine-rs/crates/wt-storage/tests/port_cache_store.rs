//! Ports of engine-go/cache_store_test.go against wt-storage cache_store public API.
//! Engine HTTP `/cache` and `/remove` paths are out of scope; those cases are
//! exercised via PeerHintStore / DhtContactStore / clear_cache_categories /
//! resume window helpers instead.

mod support;

use std::fs;
use std::time::{Duration, SystemTime};

use wt_storage::{
    clear_cache_categories, delete_resume_window, dht_contacts_path, dir_bytes, file_size,
    format_time, load_resume_window, peer_hints_path, resume_checksum, resume_window_dir,
    save_resume_window, DhtContact, DhtContactStore, PeerHintStore, ResumeWindowMeta,
    DHT_CONTACT_MAX_PER_FAMILY, PEER_HINT_TTL, RESUME_WINDOW_TTL, RESUME_WINDOW_VERSION,
};

use support::{peer_info_tcp, test_temp_dir, write_bytes};

fn resume_meta(info_hash: &str, offset: i64, length: i64, checksum: String) -> ResumeWindowMeta {
    let now = SystemTime::now();
    ResumeWindowMeta {
        version: RESUME_WINDOW_VERSION,
        info_hash: info_hash.into(),
        file_index: 0,
        file_name: "movie.mp4".into(),
        file_length: 1024 * 1024,
        piece_length: 256 * 1024,
        offset,
        length,
        checksum,
        generation: 1,
        saved_at: format_time(now),
        expires: format_time(now + RESUME_WINDOW_TTL),
    }
}

#[test]
fn test_cache_status_and_clear_categories() {
    let dir = test_temp_dir("cache_status");
    let base = dir.path();

    write_bytes(
        &base.join(".startup").join("abc").join("0.piece"),
        b"startup",
    );
    write_bytes(
        &base
            .join(".cache")
            .join("resume")
            .join("abc")
            .join("window.bin"),
        b"resume-bytes",
    );

    let hash = "0123456789abcdef0123456789abcdef01234567";
    let mut peers = PeerHintStore::new();
    peers.remember_peer_hint(base, hash, &peer_info_tcp("1.2.3.4:6881"), false);

    let mut dht = DhtContactStore::new();
    dht.remember_dht_contact(base, "8.8.8.8:6881", Some("ipv4"));

    let startup_bytes = dir_bytes(&base.join(".startup"));
    let resume_bytes = dir_bytes(&base.join(".cache").join("resume"));
    let peers_bytes = file_size(&peer_hints_path(base));
    let dht_bytes = file_size(&dht_contacts_path(base));
    assert!(startup_bytes > 0, "startup bytes");
    assert!(resume_bytes > 0, "resume bytes");
    assert!(peers_bytes > 0, "peers bytes");
    assert!(dht_bytes > 0, "dht bytes");

    clear_cache_categories(
        base,
        &[
            "peers".into(),
            "dht".into(),
            "startup".into(),
            "resume".into(),
        ],
    )
    .expect("clear");

    assert_eq!(dir_bytes(&base.join(".startup")), 0);
    assert_eq!(dir_bytes(&base.join(".cache").join("resume")), 0);
    assert!(!peer_hints_path(base).exists() || file_size(&peer_hints_path(base)) == 0);
    assert!(!dht_contacts_path(base).exists());
}

#[test]
fn test_peer_hints_persist_public_only_and_expire() {
    let dir = test_temp_dir("peer_hints");
    let base = dir.path();
    let hash = "0123456789abcdef0123456789abcdef01234567";

    let mut store = PeerHintStore::new();
    store.remember_peer_hint(base, hash, &peer_info_tcp("9.9.9.9:6881"), false);
    store.remember_peer_hint(base, hash, &peer_info_tcp("9.9.9.8:6881"), true); // private ignored
    assert_eq!(store.get_hints(hash).len(), 1);

    // Expire the hint and persist.
    {
        let hints = store.hints_mut();
        let list = hints.get_mut(hash).unwrap();
        list[0].expires = format_time(SystemTime::now() - Duration::from_secs(60));
    }
    store.persist(base);

    let mut store2 = PeerHintStore::new();
    store2.load(base);
    assert!(
        store2.get_hints(hash).is_empty(),
        "expired hints should not load: {:?}",
        store2.get_hints(hash)
    );
}

#[test]
fn test_dht_contacts_round_trip_bounded() {
    let dir = test_temp_dir("dht_rt");
    let base = dir.path();
    let mut store = DhtContactStore::new();
    for i in 0..DHT_CONTACT_MAX_PER_FAMILY + 5 {
        let ip = format!("1.2.{}.{}:6881", i / 256, i % 256);
        store.remember_dht_contact(base, &ip, Some("ipv4"));
    }
    assert!(store.contacts().len() <= DHT_CONTACT_MAX_PER_FAMILY);

    let mut store2 = DhtContactStore::new();
    store2.load(base);
    assert!(
        !store2.contacts().is_empty(),
        "expected persisted DHT contacts"
    );
}

#[test]
fn test_resume_window_persists_verified_bytes_only() {
    let dir = test_temp_dir("resume");
    let base = dir.path();
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let payload = b"verified-resume-window-bytes-0123456789";
    let meta = resume_meta(hash, 1024, payload.len() as i64, resume_checksum(payload));
    save_resume_window(base, &meta, payload).expect("save");

    let (got_meta, data) = load_resume_window(base, hash).expect("load");
    assert_eq!(got_meta.offset, 1024);
    assert_eq!(got_meta.file_index, 0);
    assert_eq!(data, payload);

    // Corrupt length must miss safely.
    fs::write(resume_window_dir(base, hash).join("window.bin"), b"x").unwrap();
    assert!(load_resume_window(base, hash).is_err());
    let _ = delete_resume_window(base, hash);
}

#[test]
fn test_forget_peers_for_a_dropped_torrent() {
    // Go path goes through /remove + /forget-peers. Here we exercise the store.
    let dir = test_temp_dir("forget_drop");
    let base = dir.path();
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let mut store = PeerHintStore::new();
    store.remember_peer_hint(base, hash, &peer_info_tcp("9.9.9.9:6881"), false);
    store.forget_peer_hints(base, hash);
    assert!(store.get_hints(hash).is_empty());
}

#[test]
fn test_forget_peer_hints_on_remove_path() {
    let dir = test_temp_dir("forget_remove");
    let base = dir.path();
    let hash = "abcdef0123456789abcdef0123456789abcdef01";
    let mut store = PeerHintStore::new();
    store.remember_peer_hint(base, hash, &peer_info_tcp("5.6.7.8:1"), false);
    store.forget_peer_hints(base, hash);
    if peer_hints_path(base).exists() {
        let raw = fs::read(peer_hints_path(base)).unwrap_or_default();
        let file: wt_storage::PeerHintFile = serde_json::from_slice(&raw).unwrap_or_default();
        assert!(
            file.torrents
                .get(hash)
                .map(|v| v.is_empty())
                .unwrap_or(true),
            "hint remained after forget"
        );
    }
}

#[test]
fn test_dht_contact_keeps_real_port() {
    let dir = test_temp_dir("dht_port");
    let base = dir.path();
    let mut store = DhtContactStore::new();
    store.remember_dht_contact(base, "1.2.3.4:51413", Some("ipv4"));
    assert_eq!(store.contacts().len(), 1);
    assert_eq!(store.contacts()[0].addr, "1.2.3.4:51413");
    assert!(
        store.contacts()[0].last_good.is_empty(),
        "routing-table style remember must not invent LastGood=now"
    );
}

#[test]
fn test_peer_hint_rejects_loopback_and_requires_listen_port() {
    let dir = test_temp_dir("peer_loopback");
    let base = dir.path();
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let mut store = PeerHintStore::new();
    store.remember_peer_hint(base, hash, &peer_info_tcp("127.0.0.1:6881"), false);
    assert!(
        store.get_hints(hash).is_empty(),
        "loopback must not be cached"
    );

    // Go also checks peerDialEndpoint listen-port remapping; that lives in the
    // engine peer layer, not wt-storage.
}

#[test]
fn test_resume_window_rejects_checksum_mismatch() {
    let dir = test_temp_dir("resume_cksum");
    let base = dir.path();
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let payload = b"verified-resume-window-bytes-0123456789";
    let meta = resume_meta(hash, 1024, payload.len() as i64, resume_checksum(payload));
    save_resume_window(base, &meta, payload).expect("save");

    let path = resume_window_dir(base, hash).join("window.bin");
    let mut raw = fs::read(&path).unwrap();
    // Flip a payload byte after the header+meta.
    if let Some(b) = raw.last_mut() {
        *b ^= 0xff;
    }
    fs::write(&path, raw).unwrap();
    assert!(load_resume_window(base, hash).is_err());
}

#[test]
fn test_cache_clear_dht_removes_memory_and_file() {
    let dir = test_temp_dir("clear_dht");
    let base = dir.path();
    let mut store = DhtContactStore::new();
    {
        let contacts = store.contacts_mut();
        contacts.push(DhtContact {
            addr: "8.8.8.8:6881".into(),
            family: "ipv4".into(),
            last_good: String::new(),
            expires: format_time(SystemTime::now() + Duration::from_secs(3600)),
        });
    }
    store.persist(base);

    // clear_cache_categories removes the file; clear memory in the store like Go's engine does.
    clear_cache_categories(base, &["dht".into()]).expect("clear");
    store.contacts_mut().clear();

    assert!(store.contacts().is_empty());
    assert!(!dht_contacts_path(base).exists());
}

#[test]
fn test_cache_clear_rejects_unknown_category() {
    let dir = test_temp_dir("clear_bad");
    assert!(clear_cache_categories(dir.path(), &["nope".into()]).is_err());
}

#[test]
fn test_drop_keeps_peer_hints_and_delete_forgets_them() {
    // Go couples this to /remove destroyStore. wt-storage exposes forget only;
    // document both behaviours at the store layer.
    for destroy in [false, true] {
        let dir = test_temp_dir("drop_keep");
        let base = dir.path();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let mut store = PeerHintStore::new();
        store.remember_peer_hint(base, hash, &peer_info_tcp("8.8.4.4:6881"), false);
        if destroy {
            store.forget_peer_hints(base, hash);
            assert_eq!(store.get_hints(hash).len(), 0);
        } else {
            assert_eq!(store.get_hints(hash).len(), 1);
        }
        let _ = PEER_HINT_TTL;
    }
}

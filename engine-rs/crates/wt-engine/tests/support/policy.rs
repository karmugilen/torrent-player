//! Real idle TCP peers for policy integration; no payload is offered.
use super::TestEngine;
use base64::Engine as _;
use std::time::{Duration, Instant};

pub fn peer() -> (TestEngine, String) {
    let eng = TestEngine::new();
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../engine-go/testdata/tiny.torrent"),
    )
    .unwrap();
    let (code, raw) = eng.dispatch("POST", "/add", &serde_json::json!({"prepare":true,"torrentData":base64::engine::general_purpose::STANDARD.encode(bytes)}).to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let id = serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, raw) = eng.dispatch("POST", "/configure", &serde_json::json!({"id":id,"descriptors":[null],"selected":[0],"storageMode":"stream","streamCacheBytes":16<<20}).to_string());
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&raw));
    let record = eng.server.records.read()[&id].clone();
    record.read().torrent.files()[0].download();
    (eng, id)
}

pub fn connect(eng: &TestEngine, id: &str, remote: &TestEngine) {
    let torrent = eng.server.records.read()[id].read().torrent.clone();
    let address = format!("127.0.0.1:{}", remote.server.client.local_port());
    torrent.add_peers(vec![candidate(&address)]);
}
pub fn candidate(address: &str) -> wt_types::PeerInfo {
    wt_types::PeerInfo {
        id: None,
        addr: address.parse().unwrap(),
        source: wt_types::PeerSource::Direct,
        supports_encryption: false,
        pex: Default::default(),
        trusted: false,
    }
}
pub fn wait_connections(eng: &TestEngine, id: &str, count: usize) {
    let torrent = eng.server.records.read()[id].read().torrent.clone();
    let deadline = Instant::now() + Duration::from_secs(3);
    while torrent.peer_conns().len() < count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        torrent.peer_conns().len(),
        count,
        "local peers did not connect"
    );
}

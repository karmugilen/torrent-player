//! Multi-peer swarm playback integration tests ported faithfully from `engine-go/direct_playback_test.go`.
//!
//! Tests Direct Play seeking beyond RAM budget and Claude seek with slow and fast seeds.

mod support;

use std::net::SocketAddr;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::json;
use support::TestEngine;

/// Helper representing one peer in a local swarm (seed or leech).
struct LocalSwarmPeer {
    engine: TestEngine,
    id: String,
    _data_file: Option<std::fs::File>,
}

impl LocalSwarmPeer {
    fn new(torrent_b64: &str, data: &[u8], seed: bool, stream_cache_bytes: usize) -> Self {
        let engine = TestEngine::new();
        // The policy sampler applies the global RAM setting to stream storage.
        // Set it as well as configure's cache so this actually crosses the RAM bound.
        engine.post_settings(&json!({"ramCacheBytes": stream_cache_bytes}).to_string());

        let (code, raw) = engine.dispatch(
            "POST",
            "/add",
            &json!({"prepare": true, "torrentData": torrent_b64}).to_string(),
        );
        assert_eq!(code, 200, "add: {}", String::from_utf8_lossy(&raw));
        let added: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let id = added["id"].as_str().unwrap().to_owned();

        let (configure_req, data_file) = if seed {
            let (code, response) = engine.dispatch(
                "POST", "/settings",
                // This fixture must seed across repeated reads and duplicate requests.
                &json!({"seedingEnabled": true, "seedingAllowed": true, "seedRatioMilli": 10000}).to_string(),
            );
            assert_eq!(code, 200, "seed settings: {}", String::from_utf8_lossy(&response));
            let file_path = engine.download_dir.join("seed_payload.dat");
            let f = std::fs::OpenOptions::new()
                .create(true).truncate(true).read(true).write(true)
                .open(&file_path).expect("create seed file");
            let fd = f.as_raw_fd();
            let req = json!({
                "id": id,
                "descriptors": [fd],
                "storageMode": "download",
                "selected": [0],
            });
            (req, Some(f))
        } else {
            let req = json!({
                "id": id,
                "descriptors": [null],
                "storageMode": "stream",
                "streamCacheBytes": stream_cache_bytes,
                "selected": [0],
            });
            (req, None)
        };

        let (code, resp) = engine.dispatch("POST", "/configure", &configure_req.to_string());
        assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&resp));

        if seed {
            let num_pieces = {
                let records = engine.server.records.read();
                let rec = records.get(&id).unwrap().read();
                rec.torrent.num_pieces()
            };
            for p in 0..num_pieces {
                support::write_verified_piece(&engine, &id, data, p as usize);
            }
        }

        Self {
            engine,
            id,
            _data_file: data_file,
        }
    }

    fn add_peer(&self, addr: SocketAddr) {
        let records = self.engine.server.records.read();
        let rec = records.get(&self.id).unwrap().read();
        rec.torrent.add_peers(vec![wt_types::PeerInfo {
            id: None,
            addr,
            source: wt_types::PeerSource::Tracker,
            supports_encryption: false,
            pex: wt_types::PexFlags::default(),
            trusted: false,
        }]);
    }

    fn wait_for_peer_connection(&self, timeout: Duration) {
        let start = Instant::now();
        loop {
            let conns = {
                let records = self.engine.server.records.read();
                let rec = records.get(&self.id).unwrap().read();
                rec.torrent.peer_conns()
            };
            if !conns.is_empty() {
                return;
            }
            if start.elapsed() > timeout {
                panic!("timed out waiting for peer connection");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn close(&self) {
        self.engine.close();
    }
}

fn build_40mib_swarm_fixture() -> (Vec<u8>, String) {
    let total = 40 * 1024 * 1024usize;
    let piece_len = 64 * 1024i64;
    let mut data = vec![0u8; total];
    for (i, b) in data.iter_mut().enumerate() {
        *b = (i * 19 + i / (64 << 10)) as u8;
    }
    use sha1::{Digest, Sha1};
    let info = wt_types::Info {
        name: "movie.mp4".into(),
        private: Some(true),
        piece_length: piece_len,
        pieces: data
            .chunks(piece_len as usize)
            .flat_map(|p| Sha1::digest(p).to_vec())
            .collect(),
        files: vec![wt_types::FileInfo {
            length: total as i64,
            ..Default::default()
        }],
        ..Default::default()
    };
    let torrent = wt_meta::encode_metainfo(&wt_types::MetaInfo {
        info_bytes: wt_meta::encode_info(&info).unwrap(),
        ..Default::default()
    })
    .unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&torrent);
    (data, encoded)
}

#[test]
fn test_direct_play_local_peer_seeks_beyond_ram_budget() {
    // Port of Go `TestDirectPlayLocalPeerSeeksBeyondRAMBudget`.
    let (data, torrent_b64) = build_40mib_swarm_fixture();

    let seed = LocalSwarmPeer::new(&torrent_b64, &data, true, 16 << 20);
    let leech = LocalSwarmPeer::new(&torrent_b64, &data, false, 16 << 20);

    // Verify Direct Play scheduled no full-file download priority
    {
        let records = leech.engine.server.records.read();
        let rec = records.get(&leech.id).unwrap().read();
        let prio = rec.torrent.files()[0].priority();
        assert_eq!(
            prio,
            wt_types::PiecePriority::None,
            "direct play scheduled full file"
        );
    }

    // Connect leech to seed
    for addr in seed.engine.server.client.listen_addrs() {
        leech.add_peer(addr);
    }
    leech.wait_for_peer_connection(Duration::from_secs(5));

    let p = leech
        .engine
        .open_playback(&leech.id, 0)
        .expect("open_playback");

    for offset in [0, 25 << 20, 4 << 20, 32 << 20, 4 << 20] {
        println!("reading Direct Play at {} MiB", offset >> 20);
        let mut got = vec![0u8; 8 << 20];
        let p_clone = p.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = p_clone.read_at(&mut got, offset);
            let _ = tx.send((res, got));
        });

        let (res, got) = rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| {
                let records = leech.engine.server.records.read();
                let rec = records.get(&leech.id).unwrap().read();
                let storage = leech
                    .engine
                    .server
                    .storage
                    .get_storage(&rec.info_hash)
                    .unwrap();
                eprintln!(
                    "[DEBUG-play] peers={:?} requests={:?} dirty={:?}",
                    rec.torrent.peer_conns(),
                    *rec.torrent.inner.request_state.read(),
                    *rec.torrent.inner.dirty_chunks.read()
                );
                panic!(
                    "read stalled at {offset}: stats={:?} cache={} max={} window={:?} pieces={:?}",
                    rec.torrent.stats(),
                    storage.cache_bytes(),
                    storage.max_cache_bytes(),
                    p.window_snapshot(),
                    rec.torrent.piece_state_runs()
                );
            });
        let n = res.expect("read_at");
        assert_eq!(n, got.len(), "short read at offset {offset}");
        assert_eq!(
            &got[..],
            &data[offset as usize..offset as usize + got.len()],
            "incorrect bytes after seek to {offset}"
        );
    }

    p.close();
    leech.close();
    seed.close();
}

#[test]
fn test_claude_seek_with_slow_and_fast_seed() {
    // Port of Go `TestClaudeSeekWithSlowAndFastSeed`.
    let (data, torrent_b64) = build_40mib_swarm_fixture();

    for claude in [false, true] {
        let slow_seed = LocalSwarmPeer::new(&torrent_b64, &data, true, 16 << 20);
        let fast_seed = LocalSwarmPeer::new(&torrent_b64, &data, true, 16 << 20);

        // Throttle slow seed to 48 KiB/s
        let (code, resp) = slow_seed.engine.dispatch(
            "POST",
            "/settings",
            &json!({"uploadRateBytesSec": 48 * 1024}).to_string(),
        );
        assert_eq!(
            code,
            200,
            "slow seed settings: {}",
            String::from_utf8_lossy(&resp)
        );

        let leech = LocalSwarmPeer::new(&torrent_b64, &data, false, 16 << 20);

        if claude {
            let (code, resp) =
                leech
                    .engine
                    .dispatch("POST", "/settings", r#"{"claudeSeekMode":true}"#);
            assert_eq!(
                code,
                200,
                "enable claude seek: {}",
                String::from_utf8_lossy(&resp)
            );
        }

        // Slow seed connects first so it is offered opening requests
        for addr in slow_seed.engine.server.client.listen_addrs() {
            leech.add_peer(addr);
        }
        leech.wait_for_peer_connection(Duration::from_secs(5));

        std::thread::sleep(Duration::from_millis(300));

        // Fast seed connects second
        for addr in fast_seed.engine.server.client.listen_addrs() {
            leech.add_peer(addr);
        }

        let p = leech
            .engine
            .open_playback(&leech.id, 0)
            .expect("open_playback");
        let start = Instant::now();

        for &offset in &[0i64, 20 << 20, 6 << 20, 33 << 20, 12 << 20, 27 << 20] {
            let mut got = vec![0u8; 1 << 20];
            let p_clone = p.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let res = p_clone.read_at(&mut got, offset);
                let _ = tx.send((res, got));
            });

            let (res, got) = rx
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("read at {offset} stalled"));
            let n = res.expect("read_at");
            assert_eq!(n, got.len(), "read at {offset} returned short");
            assert_eq!(
                &got[..],
                &data[offset as usize..offset as usize + got.len()],
                "incorrect bytes after seek to {offset}"
            );
        }

        println!("claude={claude} total elapsed: {:?}", start.elapsed());

        p.close();
        leech.close();
        fast_seed.close();
        slow_seed.close();
    }
}

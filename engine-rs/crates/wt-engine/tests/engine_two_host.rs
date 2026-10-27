//! Two engine-host processes on localhost:
//! One seeds a completed download, and the other adds it by magnet with x.pe
//! and reaches 100% with verified bytes.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

fn get_free_port(start: u16) -> u16 {
    for port in start..start + 500 {
        if let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{port}")) {
            drop(listener);
            return port;
        }
    }
    start
}

struct HostProcess {
    child: Child,
    port: u16,
}

impl HostProcess {
    fn spawn(port: u16, dir: &Path) -> Self {
        fs::create_dir_all(dir).expect("create download dir");
        let bin = env!("CARGO_BIN_EXE_engine-host");
        let child = Command::new(bin)
            .arg("--port")
            .arg(port.to_string())
            .arg("--path")
            .arg(dir.to_str().unwrap())
            .arg("--max-peers")
            .arg("55")
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("spawn engine-host");

        Self { child, port }
    }

    async fn wait_ready(&self, client: &reqwest::Client) -> Value {
        let start = Instant::now();
        loop {
            if let Ok(resp) = client
                .get(format!("http://127.0.0.1:{}/stats", self.port))
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(val) = resp.json::<Value>().await {
                        return val;
                    }
                }
            }
            if start.elapsed() > Duration::from_secs(15) {
                panic!("timed out waiting for engine-host on port {}", self.port);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn shutdown(&mut self, client: &reqwest::Client) {
        let _ = client
            .post(format!("http://127.0.0.1:{}/shutdown", self.port))
            .send()
            .await;
        let _ = self.child.wait();
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn build_fixture_torrent(
    name: &str,
    piece_length: i64,
    piece_count: usize,
) -> (Vec<u8>, Vec<u8>, String, i64) {
    use sha1::{Digest, Sha1};
    let total = piece_length as usize * piece_count;
    let mut data = Vec::with_capacity(total);
    for i in 0..total {
        data.push(((i * 17 + 31) % 256) as u8);
    }

    let mut pieces = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let end = (start + piece_length as usize).min(data.len());
        let digest = Sha1::digest(&data[start..end]);
        pieces.extend_from_slice(&digest);
        start = end;
    }

    let info = wt_types::Info {
        name: name.into(),
        piece_length,
        pieces,
        files: vec![wt_types::FileInfo {
            path: vec![],
            length: total as i64,
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        ..Default::default()
    };
    let info_bytes = wt_meta::encode_info(&info).expect("encode_info");
    let hash = wt_meta::info_hash_v1(&info_bytes);
    let mi = wt_types::MetaInfo {
        info_bytes,
        ..Default::default()
    };
    let torrent = wt_meta::encode_metainfo(&mi).expect("encode_metainfo");
    (torrent, data, hash.to_hex(), total as i64)
}

#[tokio::test]
async fn test_two_engine_host_transfer() {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let tmp_root = PathBuf::from("/home/kar/.cache/wt-tmp/agy-engine/two_host_test");
    let _ = fs::remove_dir_all(&tmp_root);
    fs::create_dir_all(&tmp_root).unwrap();

    let seeder_dir = tmp_root.join("seeder");
    let leecher_dir = tmp_root.join("leecher");

    let piece_length = 32 * 1024; // 32 KiB pieces
    let piece_count = 8; // 256 KiB total
    let (torrent_bytes, raw_data, info_hash, total_len) =
        build_fixture_torrent("video.mp4", piece_length, piece_count);

    // 1. Prepare seeder directory with completed data in boundary storage
    let seeder_part = seeder_dir.join(".boundary").join(&info_hash).join("0.part");
    fs::create_dir_all(seeder_part.parent().unwrap()).unwrap();
    fs::write(&seeder_part, &raw_data).unwrap();

    let f = fs::File::open(&seeder_part).unwrap();
    let (ident, _) = wt_storage::identity_from_file(&f, false, total_len, "".into());
    drop(f);

    let chk = wt_storage::PieceCheckpoint {
        info_hash: wt_meta::InfoHash::from_hex(&info_hash).unwrap().0,
        piece_length,
        num_pieces: piece_count,
        files: vec![ident],
        complete: vec![true; piece_count],
    };
    let chk_p = wt_storage::checkpoint_path(&seeder_dir, &info_hash);
    wt_storage::write_piece_checkpoint(&chk_p, &chk).unwrap();

    let seeder_port = get_free_port(21500);
    let mut seeder = HostProcess::spawn(seeder_port, &seeder_dir);
    let _seeder_stats = seeder.wait_ready(&http).await;

    // Turn on seeding in settings
    let resp = http
        .post(format!("http://127.0.0.1:{seeder_port}/settings"))
        .json(&json!({
            "seedingEnabled": true,
            "seedingAllowed": true,
            "payloadTransfersAllowed": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Add torrent to seeder
    let resp = http
        .post(format!("http://127.0.0.1:{seeder_port}/add"))
        .json(&json!({
            "prepare": false,
            "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent_bytes),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Configure seeder to verify completed data
    let resp = http
        .post(format!("http://127.0.0.1:{seeder_port}/configure"))
        .json(&json!({
            "id": info_hash,
            "selected": [0],
            "descriptors": [null],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Wait for seeder to reach 100% verified bytes
    let mut seeder_ready = false;
    for _ in 0..50 {
        let resp = http
            .get(format!(
                "http://127.0.0.1:{seeder_port}/torrent/{info_hash}"
            ))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            let t: Value = resp.json().await.unwrap();
            eprintln!(
                "Seeder status: verifiedBytes={:?} total_len={}",
                t["verifiedBytes"], total_len
            );
            if t["verifiedBytes"].as_i64() == Some(total_len) {
                seeder_ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seeder_ready, "seeder did not reach verified state");

    // Fetch seeder's listen port
    let stats = http
        .get(format!("http://127.0.0.1:{seeder_port}/stats"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let seeder_listen_port = stats["listenPort"].as_u64().expect("seeder listen port");
    assert!(seeder_listen_port > 0, "seeder listen port must be > 0");
    println!("Seeder running on control port {seeder_port}, peer listen port {seeder_listen_port}");

    // 2. Start leecher
    let leecher_port = get_free_port(22500);
    let mut leecher = HostProcess::spawn(leecher_port, &leecher_dir);
    let _ = leecher.wait_ready(&http).await;

    // Enable payload transfers on leecher
    let resp = http
        .post(format!("http://127.0.0.1:{leecher_port}/settings"))
        .json(&json!({
            "payloadTransfersAllowed": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Add torrent to leecher via magnet with x.pe pointing to seeder
    let magnet = format!(
        "magnet:?xt=urn:btih:{}&dn=video.mp4&x.pe=127.0.0.1:{}",
        info_hash, seeder_listen_port
    );
    let resp = http
        .post(format!("http://127.0.0.1:{leecher_port}/add"))
        .json(&json!({
            "torrentId": magnet,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Immediate configure call right after add, before metadata arrives (matches Android app behavior):
    let c_resp = http
        .post(format!("http://127.0.0.1:{leecher_port}/configure"))
        .json(&json!({
            "id": info_hash,
            "selected": [0],
            "descriptors": [null],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        c_resp.status(),
        200,
        "immediate /configure before metadata must succeed"
    );

    // Wait for transfer to reach 100% verified bytes
    let start = Instant::now();
    let mut final_torrent_json: Option<Value> = None;

    while start.elapsed() < Duration::from_secs(45) {
        let resp = http
            .get(format!(
                "http://127.0.0.1:{leecher_port}/torrent/{info_hash}"
            ))
            .send()
            .await
            .unwrap();
        if resp.status().is_success() {
            let t: Value = resp.json().await.unwrap();
            if t["verifiedBytes"].as_i64() == Some(total_len) {
                final_torrent_json = Some(t);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    let torrent_json =
        final_torrent_json.expect("leecher did not reach 100% verified bytes in time");

    println!(
        "/torrent/{} JSON:\n{}",
        info_hash,
        serde_json::to_string_pretty(&torrent_json).unwrap()
    );

    // Verify downloaded bytes on disk match fixture raw_data
    let leecher_part = leecher_dir
        .join(".boundary")
        .join(&info_hash)
        .join("0.part");
    assert!(leecher_part.exists(), "leecher part file must exist");
    let downloaded_bytes = fs::read(&leecher_part).expect("read downloaded file");
    assert_eq!(
        downloaded_bytes, raw_data,
        "downloaded bytes must match fixture"
    );

    // Clean shutdown
    seeder.shutdown(&http).await;
    leecher.shutdown(&http).await;
    let _ = fs::remove_dir_all(&tmp_root);
}

//! Direct Play thumbnail keyframe: a magnet added in Direct Play mode keeps
//! one keyframe piece (picked from the video's own index) in the startup
//! cache, and `/preview-state` reports that keyframe's time.

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


fn ebml(id: u64, body: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
    out.push(0x01);
    out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
    out.extend_from_slice(body);
    out
}

fn uint(id: u64, v: u64) -> Vec<u8> {
    ebml(id, &v.to_be_bytes())
}

/// A Matroska file: Info (100 s), Cues at the front, then filler.
fn mkv_fixture(total: usize, cues: &[(u64, u64)]) -> Vec<u8> {
    let info = [uint(0x2A_D7B1, 1_000_000), ebml(0x4489, &100_000f64.to_be_bytes())].concat();
    let points: Vec<u8> = cues
        .iter()
        .flat_map(|&(time, pos)| {
            ebml(0xBB, &[uint(0xB3, time), ebml(0xB7, &[uint(0xF7, 1), uint(0xF1, pos)].concat())].concat())
        })
        .collect();
    let head = [ebml(0x1549_A966, &info), ebml(0x1C53_BB6B, &points)].concat();
    // Segment with unknown size, so the rest of the file is its body.
    let mut data = [ebml(0x1A45_DFA3, &[]), vec![0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], head].concat();
    let mut i = data.len();
    while data.len() < total {
        data.push(((i * 17 + 31) % 256) as u8);
        i += 1;
    }
    data
}

#[tokio::test]
async fn direct_play_keeps_thumbnail_keyframe_in_startup_cache() {
    use sha1::{Digest, Sha1};
    let http = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
    let tmp_root = PathBuf::from("/home/kar/.cache/wt-tmp/thumb-keyframe-test");
    let _ = fs::remove_dir_all(&tmp_root);
    let (seeder_dir, leecher_dir) = (tmp_root.join("seeder"), tmp_root.join("leecher"));

    let piece_length: i64 = 256 * 1024;
    let piece_count = 64; // 16 MiB: head pieces 0-7, tail 56-63
    let total = piece_length as usize * piece_count;
    let segment = 24u64; // EBML header (12) + Segment id/size (12)
    let mib = 1u64 << 20;
    let data = mkv_fixture(total, &[(0, 64 * 1024), (25_000, 4 * mib), (50_000, 8 * mib), (75_000, 12 * mib)]);
    let keyframe_piece = ((segment + 4 * mib) / piece_length as u64) as usize;

    let mut pieces = Vec::new();
    for chunk in data.chunks(piece_length as usize) {
        pieces.extend_from_slice(&Sha1::digest(chunk));
    }
    let info = wt_types::Info {
        name: "movie.mkv".into(),
        piece_length,
        pieces,
        files: vec![wt_types::FileInfo { path: vec![], length: total as i64, torrent_offset: 0, attr: String::new(), pieces_root: None }],
        ..Default::default()
    };
    let info_bytes = wt_meta::encode_info(&info).unwrap();
    let info_hash = wt_meta::info_hash_v1(&info_bytes).to_hex();
    let torrent = wt_meta::encode_metainfo(&wt_types::MetaInfo { info_bytes, ..Default::default() }).unwrap();

    let part = seeder_dir.join(".boundary").join(&info_hash).join("0.part");
    fs::create_dir_all(part.parent().unwrap()).unwrap();
    fs::write(&part, &data).unwrap();
    let f = fs::File::open(&part).unwrap();
    let (ident, _) = wt_storage::identity_from_file(&f, false, total as i64, "".into());
    drop(f);
    wt_storage::write_piece_checkpoint(
        &wt_storage::checkpoint_path(&seeder_dir, &info_hash),
        &wt_storage::PieceCheckpoint {
            info_hash: wt_meta::InfoHash::from_hex(&info_hash).unwrap().0,
            piece_length,
            num_pieces: piece_count,
            files: vec![ident],
            complete: vec![true; piece_count],
        },
    )
    .unwrap();

    let seeder_port = get_free_port(23500);
    let mut seeder = HostProcess::spawn(seeder_port, &seeder_dir);
    seeder.wait_ready(&http).await;
    let post = |port: u16, path: &str, body: Value| {
        let http = http.clone();
        let url = format!("http://127.0.0.1:{port}{path}");
        async move { http.post(url).json(&body).send().await.unwrap() }
    };
    post(seeder_port, "/settings", json!({"seedingEnabled": true, "seedingAllowed": true, "payloadTransfersAllowed": true})).await;
    let resp = post(seeder_port, "/add", json!({"prepare": false, "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent)})).await;
    let seeder_id = resp.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_owned();
    assert_eq!(post(seeder_port, "/configure", json!({"id": seeder_id, "selected": [0], "descriptors": [null]})).await.status(), 200);
    let start = Instant::now();
    loop {
        let t: Value = http.get(format!("http://127.0.0.1:{seeder_port}/torrent/{seeder_id}")).send().await.unwrap().json().await.unwrap();
        if t["verifiedBytes"].as_i64() == Some(total as i64) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "seeder not verified");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let stats: Value = http.get(format!("http://127.0.0.1:{seeder_port}/stats")).send().await.unwrap().json().await.unwrap();
    let seeder_listen = stats["listenPort"].as_u64().unwrap();

    let leecher_port = get_free_port(24500);
    let mut leecher = HostProcess::spawn(leecher_port, &leecher_dir);
    leecher.wait_ready(&http).await;
    post(leecher_port, "/settings", json!({"payloadTransfersAllowed": true})).await;
    let magnet = format!("magnet:?xt=urn:btih:{info_hash}&dn=movie.mkv&x.pe=127.0.0.1:{seeder_listen}");
    let resp = post(leecher_port, "/add", json!({"torrentId": magnet})).await;
    let id = resp.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_owned();
    let resp = post(leecher_port, "/configure", json!({"id": id, "selected": [0], "descriptors": [null], "storageMode": "stream", "streamCacheBytes": 64 << 20})).await;
    assert_eq!(resp.status(), 200);

    let saved = leecher_dir.join(".startup").join(&info_hash).join(format!("{keyframe_piece}.piece"));
    let start = Instant::now();
    let mut keyframe_us = None;
    while start.elapsed() < Duration::from_secs(40) {
        let state: Value = http
            .get(format!("http://127.0.0.1:{leecher_port}/preview-state/{id}?fileIndex=0"))
            .send().await.unwrap().json().await.unwrap();
        keyframe_us = state["keyframeUs"].as_i64();
        if keyframe_us.is_some() && saved.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(keyframe_us, Some(25_000_000), "preview-state reports the 25 s keyframe");
    assert!(saved.exists(), "keyframe piece {keyframe_piece} kept in the startup cache");
    let body = leecher_dir.join(".startup").join(&info_hash).join("30.piece");
    assert!(!body.exists(), "other body pieces are not cached");

    seeder.shutdown(&http).await;
    leecher.shutdown(&http).await;
    let _ = fs::remove_dir_all(&tmp_root);
}

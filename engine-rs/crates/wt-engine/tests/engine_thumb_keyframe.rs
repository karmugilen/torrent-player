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

struct Swarm {
    seeder: HostProcess,
    leecher: HostProcess,
    leecher_dir: PathBuf,
    tmp_root: PathBuf,
    info_hash: String,
    id: String,
    seeder_id: String,
    keyframe_piece: usize,
}

const PIECE_LENGTH: i64 = 256 * 1024;

fn post(http: &reqwest::Client, port: u16, path: &str, body: Value) -> impl std::future::Future<Output = reqwest::Response> {
    let http = http.clone();
    let url = format!("http://127.0.0.1:{port}{path}");
    async move { http.post(url).json(&body).send().await.unwrap() }
}

/// A seeder with a 16 MiB MKV (cues at 0/25/50/75 s), and a leecher that
/// added its magnet (`prepare` = the Add screen, nothing chosen yet).
async fn swarm(http: &reqwest::Client, name: &str, port_base: u16, prepare: bool) -> Swarm {
    use sha1::{Digest, Sha1};
    let tmp_root = PathBuf::from(format!("/home/kar/.cache/wt-tmp/thumb-keyframe-{name}"));
    let _ = fs::remove_dir_all(&tmp_root);
    let (seeder_dir, leecher_dir) = (tmp_root.join("seeder"), tmp_root.join("leecher"));

    let piece_count = 64; // 16 MiB: head pieces 0-7, tail 56-63
    let total = PIECE_LENGTH as usize * piece_count;
    let segment = 24u64; // EBML header (12) + Segment id/size (12)
    let mib = 1u64 << 20;
    let data = mkv_fixture(total, &[(0, 64 * 1024), (25_000, 4 * mib), (50_000, 8 * mib), (75_000, 12 * mib)]);
    let keyframe_piece = ((segment + 4 * mib) / PIECE_LENGTH as u64) as usize;

    let mut pieces = Vec::new();
    for chunk in data.chunks(PIECE_LENGTH as usize) {
        pieces.extend_from_slice(&Sha1::digest(chunk));
    }
    let info = wt_types::Info {
        name: format!("movie-{name}.mkv"), // own info hash per test: no cross-test peers
        piece_length: PIECE_LENGTH,
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
            piece_length: PIECE_LENGTH,
            num_pieces: piece_count,
            files: vec![ident],
            complete: vec![true; piece_count],
        },
    )
    .unwrap();

    let seeder_port = get_free_port(port_base);
    let seeder = HostProcess::spawn(seeder_port, &seeder_dir);
    seeder.wait_ready(http).await;
    post(http, seeder_port, "/settings", json!({"seedingEnabled": true, "seedingAllowed": true, "payloadTransfersAllowed": true})).await;
    let resp = post(http, seeder_port, "/add", json!({"prepare": false, "torrentData": base64::engine::general_purpose::STANDARD.encode(&torrent)})).await;
    let seeder_id = resp.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_owned();
    let resp = post(http, seeder_port, "/configure", json!({"id": seeder_id, "selected": [0], "descriptors": [null]})).await;
    let status = resp.status();
    assert_eq!(status, 200, "seeder configure: {}", resp.text().await.unwrap_or_default());
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

    let leecher_port = get_free_port(port_base + 1000);
    let leecher = HostProcess::spawn(leecher_port, &leecher_dir);
    leecher.wait_ready(http).await;
    post(http, leecher_port, "/settings", json!({"payloadTransfersAllowed": true})).await;
    let magnet = format!("magnet:?xt=urn:btih:{info_hash}&dn=movie-{name}.mkv&x.pe=127.0.0.1:{seeder_listen}");
    let resp = post(http, leecher_port, "/add", json!({"torrentId": magnet, "prepare": prepare})).await;
    let id = resp.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_owned();
    Swarm { seeder, leecher, leecher_dir, tmp_root, info_hash, id, seeder_id, keyframe_piece }
}

async fn wait_keyframe(http: &reqwest::Client, port: u16, id: &str, extra: impl Fn() -> bool) -> Option<i64> {
    let start = Instant::now();
    let mut keyframe_us = None;
    while start.elapsed() < Duration::from_secs(40) {
        let state: Value = http
            .get(format!("http://127.0.0.1:{port}/preview-state/{id}?fileIndex=0"))
            .send().await.unwrap().json().await.unwrap();
        keyframe_us = state["keyframeUs"].as_i64();
        if keyframe_us.is_some() && extra() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    keyframe_us
}

#[tokio::test]
async fn direct_play_keeps_thumbnail_keyframe_in_startup_cache() {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
    let mut s = swarm(&http, "dp", 23500, false).await;
    let port = s.leecher.port;
    let resp = post(&http, port, "/configure", json!({"id": s.id, "selected": [0], "descriptors": [null], "storageMode": "stream", "streamCacheBytes": 64 << 20})).await;
    assert_eq!(resp.status(), 200);

    let saved = s.leecher_dir.join(".startup").join(&s.info_hash).join(format!("{}.piece", s.keyframe_piece));
    let keyframe_us = wait_keyframe(&http, port, &s.id, || saved.exists()).await;
    assert_eq!(keyframe_us, Some(25_000_000), "preview-state reports the 25 s keyframe");
    assert!(saved.exists(), "keyframe piece {} kept in the startup cache", s.keyframe_piece);
    let body = s.leecher_dir.join(".startup").join(&s.info_hash).join("30.piece");
    assert!(!body.exists(), "other body pieces are not cached");

    s.seeder.shutdown(&http).await;
    s.leecher.shutdown(&http).await;
    let _ = fs::remove_dir_all(&s.tmp_root);
}

/// Add screen: the magnet is pasted, nothing is chosen. The keyframe is still
/// found and fetched, without downloading the movie, and its bytes stay
/// readable for the preview decoder.
#[tokio::test]
async fn draft_gets_thumbnail_keyframe_before_any_choice() {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
    let mut s = swarm(&http, "draft", 25500, true).await;
    let port = s.leecher.port;

    let keyframe_us = wait_keyframe(&http, port, &s.id, || true).await;
    if keyframe_us.is_none() {
        let t = http.get(format!("http://127.0.0.1:{port}/torrent/{}", s.id)).send().await.unwrap().text().await.unwrap();
        let e = http.get(format!("http://127.0.0.1:{port}/events")).send().await.unwrap().text().await.unwrap();
        eprintln!("TORRENT {t}\nEVENTS {}", &e[e.len().saturating_sub(4000)..]);
    }
    assert_eq!(keyframe_us, Some(25_000_000), "draft preview-state reports the 25 s keyframe");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t: Value = http.get(format!("http://127.0.0.1:{}/torrent/{}", s.seeder.port, s.seeder_id)).send().await.unwrap().json().await.unwrap();
    let sent = t["uploaded"].as_i64().unwrap_or(0);
    assert!(sent > 0, "keyframe bytes were fetched");
    // Head 2 MiB + tail 4 MiB + one keyframe piece, never the 16 MiB movie.
    assert!(sent <= 7 << 20, "only index + keyframe, not the movie: {sent}");

    // Then Direct Play: the keyframe goes to the on-disk startup cache.
    let resp = post(&http, port, "/configure", json!({"id": s.id, "selected": [0], "descriptors": [null], "storageMode": "stream", "streamCacheBytes": 64 << 20})).await;
    let status = resp.status();
    assert_eq!(status, 200, "configure: {}", resp.text().await.unwrap_or_default());
    let saved = s.leecher_dir.join(".startup").join(&s.info_hash).join(format!("{}.piece", s.keyframe_piece));
    let keyframe_us = wait_keyframe(&http, port, &s.id, || saved.exists()).await;
    if keyframe_us.is_none() {
        let t = http.get(format!("http://127.0.0.1:{port}/torrent/{}", s.id)).send().await.unwrap().text().await.unwrap();
        let c = http.get(format!("http://127.0.0.1:{port}/connections/{}", s.id)).send().await.unwrap().text().await.unwrap();
        let e = http.get(format!("http://127.0.0.1:{port}/events")).send().await.unwrap().text().await.unwrap();
        let pc = http.get(format!("http://127.0.0.1:{port}/pieces/{}", s.id)).send().await.unwrap().text().await.unwrap();
        eprintln!("DPDBG PIECES {}", &pc[..pc.len().min(2500)]);
        eprintln!("DPDBG TORRENT {}\nDPDBG CONN {}\nDPDBG EVENTS {}", &t[..t.len().min(600)], &c[..c.len().min(1500)], e.chars().filter(|_| true).collect::<String>().replace("DhtGetPeers kind=timeout", "").chars().take(3000).collect::<String>());
    }
    assert_eq!(keyframe_us, Some(25_000_000));
    assert!(saved.exists(), "keyframe piece kept after choosing Direct Play");

    s.seeder.shutdown(&http).await;
    s.leecher.shutdown(&http).await;
    let _ = fs::remove_dir_all(&s.tmp_root);
}

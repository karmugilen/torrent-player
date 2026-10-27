//! Local swarm: Go/Rust host seeders behind latency/bandwidth proxies.

use crate::client::{AddRequest, ConfigureRequest, TorrentView};
use crate::engine::{free_port, spawn_engine, EngineProcess};
use crate::fixture::{prepare_seeder_store, torrent_b64, Fixture};
use crate::proxy::{start_proxy, ProxyHandle, SPEEDS};
use rand::{Rng, SeedableRng};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub struct Seeder {
    pub engine: EngineProcess,
    pub proxy: ProxyHandle,
    pub listen_port: u16,
    pub proxy_port: u16,
    pub torrent_id: String,
}

#[allow(dead_code)]
pub struct Swarm {
    pub seeders: Vec<Seeder>,
    pub work_root: PathBuf,
}

impl Swarm {
    pub fn peer_addrs(&self) -> Vec<(String, u16)> {
        self.seeders
            .iter()
            .map(|s| ("127.0.0.1".to_string(), s.proxy_port))
            .collect()
    }

    pub fn shutdown(self) {
        for s in self.seeders {
            s.proxy.stop();
            s.engine.shutdown();
        }
    }
}

/// Start `n` seeder host processes for `fixture`, each behind a proxy with
/// seek_search-like delay (20–199 ms) and one of four upload speeds.
pub fn start_swarm(
    engine_bin: &Path,
    work_root: &Path,
    fixture: &Fixture,
    n: usize,
    seed: u64,
) -> Result<Swarm, String> {
    std::fs::create_dir_all(work_root).map_err(|e| e.to_string())?;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut seeders = Vec::with_capacity(n);

    for i in 0..n {
        let dir = work_root.join(format!("seed-{i}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        prepare_seeder_store(&dir, fixture)?;

        let ctl = free_port()?;
        let engine = spawn_engine(engine_bin, &format!("seed-{i}"), ctl, &dir, 64)?;
        let client = engine.client();

        // Add + configure download mode with null descriptors.
        let add = AddRequest {
            torrent_id: None,
            prepare: true,
            torrent_data: Some(torrent_b64(fixture)),
        };
        let added: crate::client::AddResponse = client.post_json("/add", &add)?;
        wait_ready(&client, &added.id, Duration::from_secs(60))?;

        let nfiles = fixture.files.len();
        let cfg = ConfigureRequest {
            id: added.id.clone(),
            descriptors: vec![None; nfiles],
            selected: (0..nfiles as i32).collect(),
            storage_mode: Some("download".into()),
            stream_cache_bytes: None,
        };
        client.post_ok("/configure", &cfg)?;
        // Host defaults seedingEnabled=false; completed torrents won't upload
        // until it is turned on (see engine-go/seeding.go applySeedPolicy).
        // Set high seedRatioMilli and seedTimeMinutes so seeders do not disallow
        // upload across multi-repeat benchmark runs.
        let _ = client.request(
            "POST",
            "/settings",
            Some(br#"{"seedingEnabled":true,"seedingAllowed":true,"maxPeers":64,"seedRatioMilli":10000,"seedTimeMinutes":1440}"#),
        );

        // Wait until seeding / done (checkpoint restore).
        wait_seeded(&client, &added.id, Duration::from_secs(120))?;

        let stats = engine.stats()?;
        let listen = stats.listen_port as u16;
        if listen == 0 {
            return Err("seeder listenPort is 0".into());
        }

        let delay = Duration::from_millis(20 + rng.gen_range(0..180));
        let speed = SPEEDS[i % SPEEDS.len()];
        let proxy = start_proxy("127.0.0.1", listen, delay, speed)?;

        eprintln!(
            "wt-parity: seeder {i} listen={listen} proxy={} delay={delay:?} rate={}KiB/s id={}",
            proxy.listen_port,
            speed >> 10,
            added.id
        );

        let proxy_port = proxy.listen_port;
        seeders.push(Seeder {
            engine,
            proxy,
            listen_port: listen,
            proxy_port,
            torrent_id: added.id,
        });
    }

    Ok(Swarm {
        seeders,
        work_root: work_root.to_path_buf(),
    })
}

pub fn wait_ready(
    client: &crate::client::EngineClient,
    id: &str,
    timeout: Duration,
) -> Result<TorrentView, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let tv: TorrentView = client.get_json(&format!("/torrent/{id}"))?;
        if tv.ready {
            return Ok(tv);
        }
        if let Some(err) = tv.error {
            return Err(format!("torrent error: {err}"));
        }
        if Instant::now() > deadline {
            return Err(format!("timeout waiting for metadata on {id}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn wait_seeded(
    client: &crate::client::EngineClient,
    id: &str,
    timeout: Duration,
) -> Result<TorrentView, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let tv: TorrentView = client.get_json(&format!("/torrent/{id}"))?;
        if let Some(err) = &tv.error {
            return Err(format!("torrent error: {err}"));
        }
        if tv.configured && !tv.checking && (tv.done || tv.seeding || tv.progress >= 0.999) {
            return Ok(tv);
        }
        // Checkpoint restore: verified_bytes catching up.
        if tv.configured && !tv.checking && tv.verified_bytes >= tv.length && tv.length > 0 {
            return Ok(tv);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "timeout waiting for seeder ready on {id}: progress={} checking={} verified={}/{}",
                tv.progress, tv.checking, tv.verified_bytes, tv.length
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn wait_done(
    client: &crate::client::EngineClient,
    id: &str,
    timeout: Duration,
) -> Result<TorrentView, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let tv: TorrentView = client.get_json(&format!("/torrent/{id}"))?;
        if let Some(err) = &tv.error {
            return Err(format!("torrent error: {err}"));
        }
        if tv.done || (tv.length > 0 && tv.verified_bytes >= tv.length) {
            return Ok(tv);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "timeout download {id}: progress={} verified={}/{} peers={}",
                tv.progress, tv.verified_bytes, tv.length, tv.num_peers
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

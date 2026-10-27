//! Scenarios (a)–(f).

use crate::client::{
    stream_range, AddRequest, ConfigureRequest, PlayRequest, PlayResponse, StatsResponse,
    TorrentView,
};
use crate::engine::{free_port, spawn_engine, EngineProcess};
use crate::fixture::{
    ensure_fixtures, load_seek_fixture, magnet_with_peers, torrent_b64, Fixture, Scale,
};
use crate::procstat::{spawn_rss_poller, ProcMonitor};
use crate::report::{MetricSeries, RunReport};
use crate::swarm::{start_swarm, wait_done, wait_ready, Swarm};
use rand::{Rng, SeedableRng};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
pub struct RunOpts {
    pub engine: PathBuf,
    pub label: String,
    pub scenario: String,
    pub out: PathBuf,
    pub work: PathBuf,
    pub repeats: u32,
    pub seeders: usize,
}

pub fn run(opts: RunOpts) -> Result<RunReport, String> {
    let scale = Scale::from_env();
    let fixtures_dir = opts.work.join("fixtures");
    let (video, _multi) = ensure_fixtures(&fixtures_dir, scale)?;
    let seek = load_seek_fixture(&fixtures_dir, scale)?;

    let scenarios: Vec<&str> = if opts.scenario == "all" {
        vec![
            "download",
            "direct_play",
            "cold_seek",
            "magnet_metadata",
            "resources",
            "control_latency",
        ]
    } else {
        vec![opts.scenario.as_str()]
    };

    // One swarm shared across scenarios for this run when possible.
    let swarm_root = opts.work.join(format!("swarm-{}", opts.label));
    let swarm = start_swarm(&opts.engine, &swarm_root, &video, opts.seeders, 1)?;

    let started = unix_ms();
    let mut metrics = BTreeMap::new();
    let mut notes = vec![
        format!(
            "scale video={}B seek={}B piece={}",
            scale.video_bytes, scale.seek_bytes, scale.piece_length
        ),
        format!("seeders={}", opts.seeders),
    ];

    for sc in scenarios {
        eprintln!("wt-parity: scenario {sc} × {}", opts.repeats);
        match sc {
            "download" => {
                let samples = repeat(opts.repeats, |i| {
                    scenario_download(&opts, &swarm, &video, i)
                })?;
                metrics.insert(
                    "download_throughput_mib_s".into(),
                    MetricSeries::from_samples("MiB/s", samples, true),
                );
            }
            "direct_play" => {
                let mut ttfb = Vec::new();
                let mut rate = Vec::new();
                for i in 0..opts.repeats {
                    let (t, r) = scenario_direct_play(&opts, &swarm, &video, i)?;
                    ttfb.push(t);
                    rate.push(r);
                }
                metrics.insert(
                    "direct_play_ttfb_ms".into(),
                    MetricSeries::from_samples("ms", ttfb, false),
                );
                metrics.insert(
                    "direct_play_read_mib_s".into(),
                    MetricSeries::from_samples("MiB/s", rate, true),
                );
            }
            "cold_seek" => {
                // Dedicated seek swarm on the smaller fixture.
                let seek_swarm_root = opts.work.join(format!("swarm-seek-{}", opts.label));
                let seek_swarm =
                    start_swarm(&opts.engine, &seek_swarm_root, &seek, opts.seeders, 2)?;
                let mut p50 = Vec::new();
                let mut p90 = Vec::new();
                let mut mx = Vec::new();
                let mut score = Vec::new();
                for i in 0..opts.repeats {
                    let (a, b, c, s) = scenario_cold_seek(&opts, &seek_swarm, &seek, i)?;
                    p50.push(a);
                    p90.push(b);
                    mx.push(c);
                    score.push(s);
                }
                seek_swarm.shutdown();
                metrics.insert(
                    "cold_seek_p50_ms".into(),
                    MetricSeries::from_samples("ms", p50, false),
                );
                metrics.insert(
                    "cold_seek_p90_ms".into(),
                    MetricSeries::from_samples("ms", p90, false),
                );
                metrics.insert(
                    "cold_seek_max_ms".into(),
                    MetricSeries::from_samples("ms", mx, false),
                );
                metrics.insert(
                    "cold_seek_score_ms".into(),
                    MetricSeries::from_samples("ms", score, false),
                );
            }
            "magnet_metadata" => {
                let samples = repeat(opts.repeats, |i| {
                    scenario_magnet_metadata(&opts, &swarm, &video, i)
                })?;
                metrics.insert(
                    "magnet_metadata_ms".into(),
                    MetricSeries::from_samples("ms", samples, false),
                );
            }
            "resources" => {
                let mut rss_a = Vec::new();
                let mut cpu_a = Vec::new();
                let mut rss_b = Vec::new();
                let mut cpu_b = Vec::new();
                for i in 0..opts.repeats {
                    let (a, b, c, d) = scenario_resources(&opts, &swarm, &video, i)?;
                    rss_a.push(a); cpu_a.push(b); rss_b.push(c); cpu_b.push(d);
                }
                metrics.insert("download_peak_rss_mib".into(), MetricSeries::from_samples("MiB", rss_a, false));
                metrics.insert("download_cpu_seconds".into(), MetricSeries::from_samples("s", cpu_a, false));
                metrics.insert("direct_play_peak_rss_mib".into(), MetricSeries::from_samples("MiB", rss_b, false));
                metrics.insert("direct_play_cpu_seconds".into(), MetricSeries::from_samples("s", cpu_b, false));
            }
            "control_latency" => {
                let (stats_ms, torrent_ms) = scenario_control_latency(&opts, &swarm, &video)?;
                metrics.insert(
                    "control_stats_latency_ms".into(),
                    MetricSeries::from_samples("ms", stats_ms, false),
                );
                metrics.insert(
                    "control_torrent_latency_ms".into(),
                    MetricSeries::from_samples("ms", torrent_ms, false),
                );
            }
            other => return Err(format!("unknown scenario {other}")),
        }
    }

    swarm.shutdown();
    let finished = unix_ms();
    Ok(RunReport {
        label: opts.label,
        engine: opts.engine.display().to_string(),
        scenario: opts.scenario,
        started_unix_ms: started,
        finished_unix_ms: finished,
        repeats: opts.repeats,
        metrics,
        notes,
    })
}

fn repeat<F>(n: u32, mut f: F) -> Result<Vec<f64>, String>
where
    F: FnMut(u32) -> Result<f64, String>,
{
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        out.push(f(i)?);
    }
    Ok(out)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn spawn_leech(opts: &RunOpts, tag: &str, i: u32) -> Result<EngineProcess, String> {
    let dir = opts.work.join(format!("leech-{}-{}-{i}", opts.label, tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let port = free_port()?;
    spawn_engine(&opts.engine, &format!("leech-{tag}-{i}"), port, &dir, 64)
}

fn add_and_configure_download(
    leech: &EngineProcess,
    fixture: &Fixture,
    peers: &[(String, u16)],
) -> Result<String, String> {
    let client = leech.client();
    // Prefer magnet with x.pe so discovery stays local.
    let magnet = magnet_with_peers(&fixture.info_hash, &fixture.name, peers);
    let add = AddRequest {
        torrent_id: Some(magnet),
        prepare: true,
        torrent_data: None,
    };
    let added: crate::client::AddResponse = match client.post_json("/add", &add) {
        Ok(a) => a,
        Err(_) => {
            // Fallback: torrent bytes + later peer hints via re-add magnet not available;
            // use torrentData and hope DHT/PEX off; inject peers by also posting torrent.
            let add = AddRequest {
                torrent_id: None,
                prepare: true,
                torrent_data: Some(torrent_b64(fixture)),
            };
            client.post_json("/add", &add)?
        }
    };
    wait_ready(&client, &added.id, Duration::from_secs(120))?;
    let cfg = ConfigureRequest {
        id: added.id.clone(),
        descriptors: vec![None; fixture.files.len()],
        selected: (0..fixture.files.len() as i32).collect(),
        storage_mode: Some("download".into()),
        stream_cache_bytes: None,
    };
    client.post_ok("/configure", &cfg)?;
    Ok(added.id)
}

fn scenario_download(
    opts: &RunOpts,
    swarm: &Swarm,
    fixture: &Fixture,
    i: u32,
) -> Result<f64, String> {
    let leech = spawn_leech(opts, "dl", i)?;
    let peers = swarm.peer_addrs();
    let t0 = Instant::now();
    let id = add_and_configure_download(&leech, fixture, &peers)?;
    let tv = wait_done(&leech.client(), &id, Duration::from_secs(600))?;
    let elapsed = t0.elapsed().as_secs_f64().max(1e-6);
    let mib = tv.length as f64 / (1024.0 * 1024.0);
    let thr = mib / elapsed;
    eprintln!(
        "  download[{i}]: {:.2} MiB in {:.2}s -> {:.2} MiB/s peers={}",
        mib, elapsed, thr, tv.num_peers
    );
    leech.shutdown();
    Ok(thr)
}

fn scenario_direct_play(
    opts: &RunOpts,
    swarm: &Swarm,
    fixture: &Fixture,
    i: u32,
) -> Result<(f64, f64), String> {
    let leech = spawn_leech(opts, "dp", i)?;
    let client = leech.client();
    let peers = swarm.peer_addrs();
    let magnet = magnet_with_peers(&fixture.info_hash, &fixture.name, &peers);
    let add = AddRequest {
        torrent_id: Some(magnet),
        prepare: true,
        torrent_data: None,
    };
    let added: crate::client::AddResponse = client.post_json("/add", &add).or_else(|_| {
        client.post_json(
            "/add",
            &AddRequest {
                torrent_id: None,
                prepare: true,
                torrent_data: Some(torrent_b64(fixture)),
            },
        )
    })?;
    wait_ready(&client, &added.id, Duration::from_secs(120))?;
    let cfg = ConfigureRequest {
        id: added.id.clone(),
        descriptors: vec![None; fixture.files.len()],
        selected: vec![0],
        storage_mode: Some("stream".into()),
        stream_cache_bytes: Some(16 << 20),
    };
    client.post_ok("/configure", &cfg)?;

    let play: PlayResponse = client.post_json(
        "/play",
        &PlayRequest {
            id: added.id.clone(),
            file_index: Some(0),
            offset: Some(0),
        },
    )?;
    let stream_port = leech.stats()?.stream_port as u16;
    let path = play
        .stream_url
        .split("://")
        .nth(1)
        .and_then(|s| s.split_once('/').map(|(_, p)| format!("/{p}")))
        .unwrap_or_else(|| format!("/torrent/{}/file/{}", play.id, play.file_index));

    // Time to first byte: Range first 64 KiB.
    let t0 = Instant::now();
    let first = stream_range(
        stream_port,
        &path,
        0,
        Some(64 * 1024 - 1),
        Duration::from_secs(120),
    )?;
    let ttfb_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if first.len() < 1024 {
        return Err(format!("ttfb short read {}", first.len()));
    }

    // Sustained sequential read of up to 8 MiB (or file length).
    let want = (8u64 << 20).min(play.length as u64);
    let t1 = Instant::now();
    let mut got = 0u64;
    let mut off = 0u64;
    while got < want {
        let end = (off + 256 * 1024 - 1).min(want - 1);
        let chunk = stream_range(stream_port, &path, off, Some(end), Duration::from_secs(120))?;
        if chunk.is_empty() {
            break;
        }
        got += chunk.len() as u64;
        off += chunk.len() as u64;
    }
    let secs = t1.elapsed().as_secs_f64().max(1e-6);
    let rate = (got as f64 / (1024.0 * 1024.0)) / secs;
    eprintln!("  direct_play[{i}]: ttfb={ttfb_ms:.1}ms rate={rate:.2} MiB/s got={got}");
    leech.shutdown();
    Ok((ttfb_ms, rate))
}

fn scenario_cold_seek(
    opts: &RunOpts,
    swarm: &Swarm,
    fixture: &Fixture,
    i: u32,
) -> Result<(f64, f64, f64, f64), String> {
    let leech = spawn_leech(opts, "seek", i)?;
    let client = leech.client();
    let peers = swarm.peer_addrs();
    let magnet = magnet_with_peers(&fixture.info_hash, &fixture.name, &peers);
    let added: crate::client::AddResponse = client
        .post_json(
            "/add",
            &AddRequest {
                torrent_id: Some(magnet),
                prepare: true,
                torrent_data: None,
            },
        )
        .or_else(|_| {
            client.post_json(
                "/add",
                &AddRequest {
                    torrent_id: None,
                    prepare: true,
                    torrent_data: Some(torrent_b64(fixture)),
                },
            )
        })?;
    wait_ready(&client, &added.id, Duration::from_secs(120))?;
    client.post_ok(
        "/configure",
        &ConfigureRequest {
            id: added.id.clone(),
            descriptors: vec![None; fixture.files.len()],
            selected: vec![0],
            storage_mode: Some("stream".into()),
            stream_cache_bytes: Some(16 << 20),
        },
    )?;
    // Enable Claude seek tuning like the phone.
    let _ = client.request(
        "POST",
        "/settings",
        Some(br#"{"ghostProMode":true,"claudeSeekMode":true,"maxPeers":64}"#),
    );

    let play: PlayResponse = client.post_json(
        "/play",
        &PlayRequest {
            id: added.id.clone(),
            file_index: Some(0),
            offset: Some(0),
        },
    )?;
    let stream_port = leech.stats()?.stream_port as u16;
    let path = format!("/torrent/{}/file/{}", play.id, play.file_index);

    // Wait until some peers are connected.
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let tv: TorrentView = client.get_json(&format!("/torrent/{}", added.id))?;
        if tv.num_peers >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let mut rng = rand::rngs::StdRng::seed_from_u64(100 + u64::from(i));
    let piece_len = fixture.piece_length as u64;
    let file_len = fixture.total_length as u64;
    let read_len = 64u64 * 1024;
    let mut samples_ms = Vec::new();
    for _ in 0..20 {
        // Random offset into a piece, leaving room for 64 KiB.
        let max_piece = (file_len / piece_len).saturating_sub(1).max(1);
        let piece = rng.gen_range(0..max_piece);
        let within = rng.gen_range(0..piece_len.saturating_sub(read_len).max(1));
        let off = piece * piece_len + within;
        let end = off + read_len - 1;
        let t0 = Instant::now();
        match stream_range(stream_port, &path, off, Some(end), Duration::from_secs(15)) {
            Ok(buf) if buf.len() as u64 >= read_len / 2 => {
                samples_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            Ok(buf) => {
                eprintln!("  seek short read {} at off={off}", buf.len());
                samples_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            Err(e) => {
                eprintln!("  seek err at off={off}: {e}");
                samples_ms.push(15_000.0);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Fix Equal
    let p50 = crate::report::quantile(&samples_ms, 0.5);
    let p90 = crate::report::quantile(&samples_ms, 0.9);
    let max = samples_ms.last().copied().unwrap_or(0.0);
    let score = (p50 + p90) / 2.0;
    eprintln!("  cold_seek[{i}]: p50={p50:.1} p90={p90:.1} max={max:.1} score={score:.1}");
    leech.shutdown();
    Ok((p50, p90, max, score))
}

fn scenario_magnet_metadata(
    opts: &RunOpts,
    swarm: &Swarm,
    fixture: &Fixture,
    i: u32,
) -> Result<f64, String> {
    let leech = spawn_leech(opts, "mag", i)?;
    let client = leech.client();
    let peers = swarm.peer_addrs();
    let magnet = magnet_with_peers(&fixture.info_hash, &fixture.name, &peers);
    let t0 = Instant::now();
    let added: crate::client::AddResponse = client.post_json(
        "/add",
        &AddRequest {
            torrent_id: Some(magnet),
            prepare: true,
            torrent_data: None,
        },
    )?;
    wait_ready(&client, &added.id, Duration::from_secs(120))?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("  magnet_metadata[{i}]: {ms:.1} ms");
    leech.shutdown();
    Ok(ms)
}

fn scenario_resources(
    opts: &RunOpts,
    swarm: &Swarm,
    fixture: &Fixture,
    i: u32,
) -> Result<(f64, f64, f64, f64), String> {
    // (a) download resources
    let leech = spawn_leech(opts, "res-dl", i)?;
    let mon = ProcMonitor::start(leech.pid());
    let (stop, join) = spawn_rss_poller(leech.pid(), Duration::from_millis(200));
    let peers = swarm.peer_addrs();
    let id = add_and_configure_download(&leech, fixture, &peers)?;
    wait_done(&leech.client(), &id, Duration::from_secs(600))?;
    drop(stop);
    let peak = join.join().unwrap_or(0);
    let sample = mon.finish();
    let rss_a = peak.max(sample.peak_rss_bytes) as f64 / (1024.0 * 1024.0);
    let cpu_a = sample.cpu_seconds;
    leech.shutdown();

    // (b) direct play resources: ttfb + short sequential read
    let leech = spawn_leech(opts, "res-dp", i)?;
    let mon = ProcMonitor::start(leech.pid());
    let (stop, join) = spawn_rss_poller(leech.pid(), Duration::from_millis(200));
    scenario_direct_play_on(&leech, swarm, fixture)?;
    drop(stop);
    let peak = join.join().unwrap_or(0);
    let sample = mon.finish();
    let rss_b = peak.max(sample.peak_rss_bytes) as f64 / (1024.0 * 1024.0);
    let cpu_b = sample.cpu_seconds;
    leech.shutdown();

    eprintln!(
        "  resources: dl RSS={rss_a:.1}MiB CPU={cpu_a:.2}s; dp RSS={rss_b:.1}MiB CPU={cpu_b:.2}s"
    );
    Ok((rss_a, cpu_a, rss_b, cpu_b))
}

fn scenario_direct_play_on(
    leech: &EngineProcess,
    swarm: &Swarm,
    fixture: &Fixture,
) -> Result<(), String> {
    let client = leech.client();
    let peers = swarm.peer_addrs();
    let magnet = magnet_with_peers(&fixture.info_hash, &fixture.name, &peers);
    let added: crate::client::AddResponse = client
        .post_json(
            "/add",
            &AddRequest {
                torrent_id: Some(magnet),
                prepare: true,
                torrent_data: None,
            },
        )
        .or_else(|_| {
            client.post_json(
                "/add",
                &AddRequest {
                    torrent_id: None,
                    prepare: true,
                    torrent_data: Some(torrent_b64(fixture)),
                },
            )
        })?;
    wait_ready(&client, &added.id, Duration::from_secs(120))?;
    client.post_ok(
        "/configure",
        &ConfigureRequest {
            id: added.id.clone(),
            descriptors: vec![None; fixture.files.len()],
            selected: vec![0],
            storage_mode: Some("stream".into()),
            stream_cache_bytes: Some(16 << 20),
        },
    )?;
    let play: PlayResponse = client.post_json(
        "/play",
        &PlayRequest {
            id: added.id,
            file_index: Some(0),
            offset: Some(0),
        },
    )?;
    let stream_port = leech.stats()?.stream_port as u16;
    let path = format!("/torrent/{}/file/{}", play.id, play.file_index);
    let _ = stream_range(
        stream_port,
        &path,
        0,
        Some(1024 * 1024 - 1),
        Duration::from_secs(120),
    )?;
    Ok(())
}

fn scenario_control_latency(
    opts: &RunOpts,
    swarm: &Swarm,
    fixture: &Fixture,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let leech = spawn_leech(opts, "ctl", 0)?;
    let peers = swarm.peer_addrs();
    let id = add_and_configure_download(&leech, fixture, &peers)?;
    // Under load: while download progresses, hammer APIs.
    let client = leech.client();
    let mut stats_ms = Vec::new();
    let mut torrent_ms = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let t0 = Instant::now();
        let _: StatsResponse = client.get_json("/stats")?;
        stats_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        let t1 = Instant::now();
        let _: TorrentView = client.get_json(&format!("/torrent/{id}"))?;
        torrent_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_millis(50));
    }
    leech.shutdown();
    eprintln!(
        "  control_latency: stats n={} torrent n={}",
        stats_ms.len(),
        torrent_ms.len()
    );
    Ok((stats_ms, torrent_ms))
}

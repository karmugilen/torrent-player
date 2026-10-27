//! Go-vs-Rust control API contract checker.
//!
//! Spawns both host engines, drives every `handleControl` route from
//! `engine-go/server.go` against a local fixture, diffs HTTP status + JSON
//! shape/values, and writes `API_DIFF.md`. Fields the Kotlin app reads are
//! marked CRITICAL when they differ.

use crate::client::{EngineClient, HttpResponse};
use crate::engine::{free_port, spawn_engine};
use crate::fixture::{
    ensure_fixtures, magnet_with_peers, prepare_seeder_store, torrent_b64, Scale,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// JSON fields the Android Kotlin client actually parses (EngineClient.kt).
/// Diffs on these are CRITICAL.
pub fn kotlin_critical_fields() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let mut m: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    m.insert(
        "/stats",
        [
            "downloadSpeed",
            "uploadSpeed",
            "progress",
            "ratio",
            "torrents",
            "ctlPort",
            "streamPort",
            "path",
            "listenPort",
        ]
        .into_iter()
        .collect(),
    );
    m.insert("/add", ["id", "infoHash"].into_iter().collect());
    m.insert(
        "/torrent/<id>",
        [
            "id",
            "infoHash",
            "name",
            "magnetURI",
            "ready",
            "done",
            "paused",
            "progress",
            "downloadSpeed",
            "uploadSpeed",
            "numPeers",
            "length",
            "downloaded",
            "uploaded",
            "seeding",
            "seedRatio",
            "playbackReaders",
            "resting",
            "idleDropReady",
            "nextPreload",
            "error",
            "timeRemaining",
            "configured",
            "checking",
            "checkedPieces",
            "checkTotal",
            "focusedFile",
            "warning",
            "verifiedBytes",
            "files",
            "selected",
            "trackers",
        ]
        .into_iter()
        .collect(),
    );
    // Nested under /torrent/<id> files[]
    m.insert(
        "/torrent/<id>.files[]",
        [
            "index",
            "name",
            "path",
            "length",
            "progress",
            "type",
            "verifiedBytes",
        ]
        .into_iter()
        .collect(),
    );
    m.insert(
        "/play",
        ["id", "fileIndex", "name", "length", "streamUrl"]
            .into_iter()
            .collect(),
    );
    m.insert(
        "/pieces/<id>",
        [
            "id",
            "infoHash",
            "generation",
            "timestamp",
            "totalPieces",
            "pieceLength",
            "lastPieceLength",
            "maxBuckets",
            "buckets",
            "ghostProMode",
            "playback",
            "playheads",
            "playbackDetail",
        ]
        .into_iter()
        .collect(),
    );
    m.insert("/metadata/<id>", ["torrentData"].into_iter().collect());
    m.insert(
        "/settings",
        [
            "maxPeers",
            "ramCacheBytes",
            "downloadRateBytesSec",
            "uploadRateBytesSec",
            "dialRatePerSec",
            "peerExploration",
            "ghostMode",
            "ghostProMode",
            "seekLogPath",
            "seekLogTag",
            "seekExperiment",
            "idleDropMinutes",
            "claudeSeekMode",
            "hedgeRequests",
            "slowPeerYield",
            "dropChokers",
            "seekStats",
            "arrival",
            "payloadTransfersAllowed",
            "seedingEnabled",
            "seedRatioMilli",
            "seedTimeMinutes",
        ]
        .into_iter()
        .collect(),
    );
    m.insert(
        "/preview-state/<id>",
        ["active", "byteOffset", "signature"].into_iter().collect(),
    );
    m.insert(
        "/connections/<id>",
        ["id", "paused", "peers"].into_iter().collect(),
    );
    m.insert(
        "/bypass",
        ["enabled", "active", "mode", "coverage", "error"]
            .into_iter()
            .collect(),
    );
    m.insert(
        "/cache",
        ["categories", "memoryBytes"].into_iter().collect(),
    );
    m.insert("/select", ["ok", "selected"].into_iter().collect());
    m
}

/// Fields whose values must match exactly when both sides succeed (ids, names,
/// lengths, file lists). Volatile counters (speeds, progress, timestamps) are
/// compared for type/presence only.
/// Values that must match when both sides return the same logical torrent.
/// Cross-engine `id` is type-checked only (Go uses UUID; Rust may use infohash).
const EXACT_VALUE_KEYS: &[&str] = &[
    "infoHash",
    "name",
    "length",
    "path", // file path inside torrent views (not /stats data-dir)
    "index",
    "type",
    "fileIndex",
    "totalPieces",
    "pieceLength",
    "lastPieceLength",
    "selected",
    "configured",
    "ok",
    "torrentData",
    "maxPeers",
    "ghostMode",
    "ghostProMode",
    "seedingEnabled",
    "payloadTransfersAllowed",
    "ramCacheBytes",
    "dialRatePerSec",
    "idleDropMinutes",
];

const VOLATILE_KEYS: &[&str] = &[
    "downloadSpeed",
    "uploadSpeed",
    "progress",
    "ratio",
    "numPeers",
    "downloaded",
    "uploaded",
    "verifiedBytes",
    "timestamp",
    "generation",
    "timeRemaining",
    "ctlPort",
    "streamPort",
    "listenPort",
    "seedRatio",
    "checkedPieces",
    "checkTotal",
    "playbackReaders",
    "byteOffset",
    "signature",
    "streamUrl", // port differs per host
    "id",        // per-engine identifier; type-checked only
];

#[derive(Debug, Clone)]
pub struct RouteResult {
    pub route: String,
    pub method: String,
    pub go_status: Option<u16>,
    pub rust_status: Option<u16>,
    pub verdict: Verdict,
    pub details: Vec<String>,
    pub critical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Match,
    Diff,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Match => "MATCH",
            Verdict::Diff => "DIFF",
        }
    }
}

pub struct ApiDiffOpts {
    pub go_bin: PathBuf,
    pub rust_bin: PathBuf,
    pub work: PathBuf,
    pub report: PathBuf,
    /// Repo root (parent of engine-go / engine-rs) for builds.
    pub repo: PathBuf,
    pub build: bool,
}

pub struct ApiDiffSummary {
    pub match_count: usize,
    pub diff_count: usize,
    pub critical_diffs: Vec<String>,
    pub report_path: PathBuf,
}

pub fn run_api_diff(opts: ApiDiffOpts) -> Result<ApiDiffSummary, String> {
    std::fs::create_dir_all(&opts.work).map_err(|e| e.to_string())?;
    let fixture_root = opts.work.join("fixtures");
    std::fs::create_dir_all(&fixture_root).map_err(|e| e.to_string())?;

    if opts.build || !opts.go_bin.exists() {
        eprintln!("api-diff: building Go engine-go-host …");
        build_go_host(&opts.repo, &opts.go_bin)?;
    }
    if opts.build || !opts.rust_bin.exists() {
        eprintln!("api-diff: building Rust engine-host …");
        build_rust_host(&opts.repo, &opts.rust_bin)?;
    }
    if !opts.go_bin.exists() {
        return Err(format!("Go binary missing: {}", opts.go_bin.display()));
    }
    if !opts.rust_bin.exists() {
        return Err(format!("Rust binary missing: {}", opts.rust_bin.display()));
    }

    // Tiny fixture keeps disk under the 256 MiB budget for this crate.
    std::env::set_var("WT_PARITY_SCALE", "tiny");
    let scale = Scale::from_env();
    let (video, _multi) = ensure_fixtures(&fixture_root, scale)?;
    let b64 = torrent_b64(&video);
    let info_hash_hex = video.info_hash.to_hex();

    let go_data = opts.work.join("go-data");
    let rust_data = opts.work.join("rust-data");
    let _ = std::fs::remove_dir_all(&go_data);
    let _ = std::fs::remove_dir_all(&rust_data);
    std::fs::create_dir_all(&go_data).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&rust_data).map_err(|e| e.to_string())?;

    // Seed fixture into Go store so metadata/add with torrentData is instant.
    prepare_seeder_store(&go_data, &video)?;

    let go_port = free_port()?;
    let rust_port = free_port()?;
    eprintln!("api-diff: starting Go host on :{go_port}");
    let go_proc = spawn_engine(&opts.go_bin, "go", go_port, &go_data, 40)?;
    eprintln!("api-diff: starting Rust host on :{rust_port}");
    let rust_proc = match spawn_engine(&opts.rust_bin, "rust", rust_port, &rust_data, 40) {
        Ok(p) => p,
        Err(e) => {
            go_proc.shutdown();
            return Err(e);
        }
    };

    let go = go_proc.client();
    let rust = rust_proc.client();
    let mut results: Vec<RouteResult> = Vec::new();

    // --- 1. GET /stats ---
    results.push(drive_pair(
        "GET",
        "/stats",
        "/stats",
        None,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    // --- 2. POST /add with torrentData ---
    let add_body = serde_json::json!({
        "torrentId": info_hash_hex,
        "prepare": true,
        "torrentData": b64,
    });
    let add_payload = serde_json::to_vec(&add_body).unwrap();
    let (go_add, rust_add) = dual_request(&go, &rust, "POST", "/add", Some(&add_payload));
    let go_id = extract_id(&go_add);
    let rust_id = extract_id(&rust_add);
    results.push(compare_responses(
        "POST",
        "/add (torrentData)",
        "/add",
        go_add,
        rust_add,
        ExactPolicy::ExactKeys(&[
            "infoHash", /* id is UUID per engine — compare presence/type only */
        ]),
        &["id"], // id type must match (string) but value may differ
    ));

    // Magnets for second add (after we have listen ports from stats).
    let go_stats = go.get_json::<Value>("/stats").ok();
    let rust_stats = rust.get_json::<Value>("/stats").ok();
    let go_listen = go_stats
        .as_ref()
        .and_then(|v| v.get("listenPort"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as u16;
    let _rust_listen = rust_stats
        .as_ref()
        .and_then(|v| v.get("listenPort"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as u16;

    // Use Go as seeder peer for magnet adds on both (same swarm fixture).
    let magnet = if go_listen > 0 {
        magnet_with_peers(
            &video.info_hash,
            &video.name,
            &[("127.0.0.1".into(), go_listen)],
        )
    } else {
        format!("magnet:?xt=urn:btih:{}&dn={}", info_hash_hex, video.name)
    };

    // --- per-torrent routes (need ids) ---
    let go_tid = go_id.clone().unwrap_or_else(|| "missing-go-id".into());
    let rust_tid = rust_id.clone().unwrap_or_else(|| "missing-rust-id".into());

    results.push(drive_pair_paths(
        "GET",
        &format!("/torrent/{go_tid}"),
        &format!("/torrent/{rust_tid}"),
        "/torrent/<id>",
        None,
        &go,
        &rust,
        ExactPolicy::ExactKeys(&["name", "length", "infoHash"]),
    ));

    // configure (stream mode — no Android FDs available out of process)
    let go_cfg = serde_json::json!({
        "id": go_tid,
        "descriptors": [null],
        "selected": [0],
        "storageMode": "stream",
        "streamCacheBytes": 8_388_608i64,
    });
    let rust_cfg = serde_json::json!({
        "id": rust_tid,
        "descriptors": [null],
        "selected": [0],
        "storageMode": "stream",
        "streamCacheBytes": 8_388_608i64,
    });
    results.push(drive_pair_bodies(
        "POST",
        "/configure",
        "/configure",
        &go_cfg,
        &rust_cfg,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    let go_sel = serde_json::json!({"id": go_tid, "selected": [0]});
    let rust_sel = serde_json::json!({"id": rust_tid, "selected": [0]});
    results.push(drive_pair_bodies(
        "POST",
        "/select",
        "/select",
        &go_sel,
        &rust_sel,
        &go,
        &rust,
        ExactPolicy::ExactKeys(&["ok", "selected"]),
    ));

    let go_play = serde_json::json!({"id": go_tid, "fileIndex": 0});
    let rust_play = serde_json::json!({"id": rust_tid, "fileIndex": 0});
    results.push(drive_pair_bodies(
        "POST",
        "/play",
        "/play",
        &go_play,
        &rust_play,
        &go,
        &rust,
        ExactPolicy::ExactKeys(&["fileIndex", "name", "length"]),
    ));

    let go_fo = serde_json::json!({"id": go_tid, "enabled": false});
    let rust_fo = serde_json::json!({"id": rust_tid, "enabled": false});
    results.push(drive_pair_bodies(
        "POST",
        "/file-only",
        "/file-only",
        &go_fo,
        &rust_fo,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    let go_focus = serde_json::json!({"id": go_tid, "fileIndex": 0});
    let rust_focus = serde_json::json!({"id": rust_tid, "fileIndex": 0});
    results.push(drive_pair_bodies(
        "POST",
        "/focus",
        "/focus",
        &go_focus,
        &rust_focus,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    let go_pause = serde_json::json!({"id": go_tid});
    let rust_pause = serde_json::json!({"id": rust_tid});
    results.push(drive_pair_bodies(
        "POST",
        "/pause",
        "/pause",
        &go_pause,
        &rust_pause,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    results.push(drive_pair_bodies(
        "POST",
        "/resume",
        "/resume",
        &go_pause,
        &rust_pause,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    let forget = serde_json::json!({"infoHash": info_hash_hex});
    results.push(drive_pair_bodies(
        "POST",
        "/forget-peers",
        "/forget-peers",
        &forget,
        &forget,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    results.push(drive_pair_paths(
        "GET",
        &format!("/pieces/{go_tid}"),
        &format!("/pieces/{rust_tid}"),
        "/pieces/<id>",
        None,
        &go,
        &rust,
        ExactPolicy::ExactKeys(&["totalPieces", "pieceLength", "lastPieceLength"]),
    ));

    results.push(drive_pair_paths(
        "GET",
        &format!("/metadata/{go_tid}"),
        &format!("/metadata/{rust_tid}"),
        "/metadata/<id>",
        None,
        &go,
        &rust,
        ExactPolicy::ExactKeys(&["torrentData"]),
    ));

    // --- settings ---
    results.push(drive_pair(
        "GET",
        "/settings",
        "/settings",
        None,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    let settings_post = serde_json::json!({
        "maxPeers": 40,
        "seedingEnabled": true,
        "payloadTransfersAllowed": true,
    });
    results.push(drive_pair_bodies(
        "POST",
        "/settings",
        "/settings",
        &settings_post,
        &settings_post,
        &go,
        &rust,
        ExactPolicy::ExactKeys(&["maxPeers", "seedingEnabled", "payloadTransfersAllowed"]),
    ));

    // --- /add with magnet (second torrent id = magnet URI) ---
    // Remove first if needed so magnet can re-add, or use prepare on magnet.
    // Prefer adding magnet on a fresh path: remove then re-add via magnet.
    let go_rm = serde_json::json!({"id": go_tid, "destroyStore": false});
    let rust_rm = serde_json::json!({"id": rust_tid, "destroyStore": false});
    // We'll record /remove later; for magnet we add without removing if engines allow re-add.
    let magnet_body = serde_json::json!({
        "torrentId": magnet,
        "prepare": true,
    });
    let magnet_payload = serde_json::to_vec(&magnet_body).unwrap();
    // If already present, Go returns existing; still fine for contract.
    let (go_mag, rust_mag) = dual_request(&go, &rust, "POST", "/add", Some(&magnet_payload));
    results.push(compare_responses(
        "POST",
        "/add (magnet)",
        "/add",
        go_mag,
        rust_mag,
        ExactPolicy::StructureOnly,
        &["id"],
    ));

    // Extra Go routes the Kotlin app also hits (still in handleControl).
    results.push(drive_pair_paths(
        "GET",
        &format!("/preview-state/{go_tid}"),
        &format!("/preview-state/{rust_tid}"),
        "/preview-state/<id>",
        None,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    results.push(drive_pair_paths(
        "GET",
        &format!("/connections/{go_tid}"),
        &format!("/connections/{rust_tid}"),
        "/connections/<id>",
        None,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    results.push(drive_pair(
        "GET",
        "/bypass",
        "/bypass",
        None,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    let bypass_post = serde_json::json!({"enabled": false});
    results.push(drive_pair_bodies(
        "POST",
        "/bypass",
        "/bypass",
        &bypass_post,
        &bypass_post,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    results.push(drive_pair(
        "GET",
        "/cache",
        "/cache",
        None,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    let cache_clear = serde_json::json!({"categories": ["all"]});
    results.push(drive_pair_bodies(
        "POST",
        "/cache/clear",
        "/cache/clear",
        &cache_clear,
        &cache_clear,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));
    let network = serde_json::json!({"generation": 1, "connected": true});
    results.push(drive_pair_bodies(
        "POST",
        "/network",
        "/network",
        &network,
        &network,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    // --- bad inputs (expect 4xx) ---
    results.push(drive_pair(
        "POST",
        "/add",
        "/add (bad: empty)",
        Some(b"{}"),
        &go,
        &rust,
        ExactPolicy::StatusClass4xx,
    ));
    results.push(drive_pair(
        "POST",
        "/add",
        "/add (bad: invalid json)",
        Some(b"not-json"),
        &go,
        &rust,
        ExactPolicy::StatusClass4xx,
    ));
    results.push(drive_pair(
        "POST",
        "/play",
        "/play (bad: missing id)",
        Some(br#"{"fileIndex":0}"#),
        &go,
        &rust,
        ExactPolicy::StatusClass4xx,
    ));
    results.push(drive_pair(
        "GET",
        "/torrent/does-not-exist",
        "/torrent/<id> (bad: missing)",
        None,
        &go,
        &rust,
        ExactPolicy::StatusClass4xx,
    ));
    results.push(drive_pair(
        "GET",
        "/no-such-route",
        "/ (bad: 404)",
        None,
        &go,
        &rust,
        ExactPolicy::StatusClass4xx,
    ));

    // --- /remove ---
    results.push(drive_pair_bodies(
        "POST",
        "/remove",
        "/remove",
        &go_rm,
        &rust_rm,
        &go,
        &rust,
        ExactPolicy::StructureOnly,
    ));

    // --- /shutdown last (kills hosts) ---
    let (go_sd, rust_sd) = dual_request(&go, &rust, "POST", "/shutdown", Some(b"{}"));
    results.push(compare_responses(
        "POST",
        "/shutdown",
        "/shutdown",
        go_sd,
        rust_sd,
        ExactPolicy::StructureOnly,
        &[],
    ));

    // Allow processes to exit; Drop also kills.
    std::thread::sleep(Duration::from_millis(200));
    drop(go_proc);
    drop(rust_proc);

    let report = render_report(&results, &info_hash_hex);
    if let Some(parent) = opts.report.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&opts.report, &report).map_err(|e| e.to_string())?;
    eprintln!("api-diff: wrote {}", opts.report.display());

    let match_count = results
        .iter()
        .filter(|r| r.verdict == Verdict::Match)
        .count();
    let diff_count = results
        .iter()
        .filter(|r| r.verdict == Verdict::Diff)
        .count();
    let critical_diffs: Vec<String> = results
        .iter()
        .filter(|r| r.verdict == Verdict::Diff && r.critical)
        .map(|r| format!("{} {}: {}", r.method, r.route, r.details.join("; ")))
        .collect();

    // Clean fixture work dir contents larger than needed? Keep for re-run;
    // task says fixtures under api-diff and deleted after tests — wipe data dirs.
    let _ = std::fs::remove_dir_all(&go_data);
    let _ = std::fs::remove_dir_all(&rust_data);

    Ok(ApiDiffSummary {
        match_count,
        diff_count,
        critical_diffs,
        report_path: opts.report,
    })
}

fn build_go_host(repo: &Path, out: &Path) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let engine_go = repo.join("engine-go");
    let status = Command::new("go")
        .current_dir(&engine_go)
        .args([
            "build",
            "-tags=nosqlite,noboltdb",
            "-o",
            out.to_str().ok_or("out path")?,
            ".",
        ])
        .status()
        .map_err(|e| format!("go build: {e}"))?;
    if !status.success() {
        return Err(format!("go build failed: {status}"));
    }
    Ok(())
}

fn build_rust_host(repo: &Path, out: &Path) -> Result<(), String> {
    let engine_rs = repo.join("engine-rs");
    let target = PathBuf::from("/home/kar/.cache/wt-targets/wt-parity");
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let status = Command::new("cargo")
        .current_dir(&engine_rs)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_BUILD_JOBS", "3")
        .args(["build", "-p", "wt-engine", "--bin", "engine-host"])
        .status()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err(format!(
            "cargo build -p wt-engine --bin engine-host failed: {status}"
        ));
    }
    let built = target.join("debug/engine-host");
    if built != *out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&built, out).map_err(|e| format!("copy engine-host: {e}"))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ExactPolicy {
    StructureOnly,
    ExactKeys(&'static [&'static str]),
    StatusClass4xx,
}

type SideResult = Result<HttpResponse, String>;

fn dual_request(
    go: &EngineClient,
    rust: &EngineClient,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> (SideResult, SideResult) {
    (
        go.request(method, path, body),
        rust.request(method, path, body),
    )
}

fn extract_id(resp: &SideResult) -> Option<String> {
    let Ok(r) = resp else { return None };
    let v: Value = serde_json::from_slice(&r.body).ok()?;
    v.get("id")?.as_str().map(|s| s.to_string())
}

fn drive_pair(
    method: &str,
    path: &str,
    label: &str,
    body: Option<&[u8]>,
    go: &EngineClient,
    rust: &EngineClient,
    policy: ExactPolicy,
) -> RouteResult {
    let (g, r) = dual_request(go, rust, method, path, body);
    compare_responses(method, label, critical_key_for(label), g, r, policy, &[])
}

#[allow(clippy::too_many_arguments)]
fn drive_pair_paths(
    method: &str,
    go_path: &str,
    rust_path: &str,
    critical_key: &str,
    body: Option<&[u8]>,
    go: &EngineClient,
    rust: &EngineClient,
    policy: ExactPolicy,
) -> RouteResult {
    let g = go.request(method, go_path, body);
    let r = rust.request(method, rust_path, body);
    let label = go_path_label(go_path, critical_key);
    compare_responses(method, &label, critical_key, g, r, policy, &[])
}

fn go_path_label(_go_path: &str, critical_key: &str) -> String {
    critical_key.to_string()
}

#[allow(clippy::too_many_arguments)]
fn drive_pair_bodies(
    method: &str,
    path: &str,
    label: &str,
    go_body: &Value,
    rust_body: &Value,
    go: &EngineClient,
    rust: &EngineClient,
    policy: ExactPolicy,
) -> RouteResult {
    let gb = serde_json::to_vec(go_body).unwrap_or_default();
    let rb = serde_json::to_vec(rust_body).unwrap_or_default();
    let g = go.request(method, path, Some(&gb));
    let r = rust.request(method, path, Some(&rb));
    compare_responses(method, label, critical_key_for(label), g, r, policy, &[])
}

fn critical_key_for(label: &str) -> &str {
    if label.starts_with("/add") {
        "/add"
    } else if label.starts_with("/torrent") {
        "/torrent/<id>"
    } else if label.starts_with("/pieces") {
        "/pieces/<id>"
    } else if label.starts_with("/metadata") {
        "/metadata/<id>"
    } else if label.starts_with("/preview-state") {
        "/preview-state/<id>"
    } else if label.starts_with("/connections") {
        "/connections/<id>"
    } else if label.starts_with("/settings") {
        "/settings"
    } else if label.starts_with("/play") {
        "/play"
    } else if label.starts_with("/select") {
        "/select"
    } else if label.starts_with("/bypass") {
        "/bypass"
    } else if label.starts_with("/cache") {
        "/cache"
    } else if label.starts_with("/stats") {
        "/stats"
    } else {
        label
    }
}

fn compare_responses(
    method: &str,
    route: &str,
    critical_key: &str,
    go: SideResult,
    rust: SideResult,
    policy: ExactPolicy,
    id_type_only: &[&str],
) -> RouteResult {
    let mut details = Vec::new();
    let (go_status, go_body) = match go {
        Ok(r) => (Some(r.status), Some(r.body)),
        Err(e) => {
            details.push(format!("go transport error: {e}"));
            (None, None)
        }
    };
    let (rust_status, rust_body) = match rust {
        Ok(r) => (Some(r.status), Some(r.body)),
        Err(e) => {
            details.push(format!("rust transport error: {e}"));
            (None, None)
        }
    };

    match policy {
        ExactPolicy::StatusClass4xx => {
            let go_ok = go_status.map(|s| (400..500).contains(&s)).unwrap_or(false);
            let rust_ok = rust_status
                .map(|s| (400..500).contains(&s))
                .unwrap_or(false);
            if go_status != rust_status {
                details.push(format!("status go={go_status:?} rust={rust_status:?}"));
            }
            // Both must be 4xx for MATCH; if both 4xx even with different codes, soft-match
            // but note the code difference.
            let verdict = if go_ok && rust_ok {
                if go_status != rust_status {
                    details.push("both 4xx but codes differ".into());
                    Verdict::Diff
                } else {
                    Verdict::Match
                }
            } else {
                if !go_ok {
                    details.push(format!("go not 4xx: {go_status:?}"));
                }
                if !rust_ok {
                    details.push(format!("rust not 4xx: {rust_status:?}"));
                }
                Verdict::Diff
            };
            let critical = verdict == Verdict::Diff;
            return RouteResult {
                route: route.to_string(),
                method: method.to_string(),
                go_status,
                rust_status,
                verdict,
                details,
                critical,
            };
        }
        _ => {
            if go_status != rust_status {
                details.push(format!("status go={go_status:?} rust={rust_status:?}"));
            }
        }
    }

    let go_json = go_body
        .as_ref()
        .and_then(|b| serde_json::from_slice::<Value>(b).ok());
    let rust_json = rust_body
        .as_ref()
        .and_then(|b| serde_json::from_slice::<Value>(b).ok());

    if go_body.is_some() && go_json.is_none() {
        details.push("go body is not JSON".into());
    }
    if rust_body.is_some() && rust_json.is_none() {
        details.push(format!(
            "rust body is not JSON: {}",
            rust_body
                .as_ref()
                .map(|b| String::from_utf8_lossy(b)
                    .chars()
                    .take(120)
                    .collect::<String>())
                .unwrap_or_default()
        ));
    }

    let mut critical = false;
    if let (Some(gj), Some(rj)) = (&go_json, &rust_json) {
        let exact = match policy {
            ExactPolicy::ExactKeys(keys) => keys,
            _ => &[],
        };
        let diffs = diff_json(gj, rj, "", exact, id_type_only);
        let crit_fields = kotlin_critical_fields()
            .get(critical_key)
            .cloned()
            .unwrap_or_default();
        // Also merge nested files[] critical set when comparing torrent.
        let mut all_crit = crit_fields;
        if critical_key == "/torrent/<id>" {
            if let Some(f) = kotlin_critical_fields().get("/torrent/<id>.files[]") {
                all_crit.extend(f.iter().copied());
            }
        }
        for d in &diffs {
            let field = d.split(':').next().unwrap_or(d);
            let leaf = field.rsplit('.').next().unwrap_or(field);
            let leaf =
                leaf.trim_start_matches(|c: char| c.is_ascii_digit() || c == '[' || c == ']');
            let leaf = leaf.trim_matches(|c| c == '[' || c == ']' || c == '.');
            let is_crit = all_crit.iter().any(|c| field.contains(c) || leaf == *c);
            if is_crit {
                critical = true;
                details.push(format!("CRITICAL {d}"));
            } else {
                details.push(d.clone());
            }
        }
    } else if go_json.is_some() != rust_json.is_some() {
        critical = true;
        details.push("CRITICAL one side missing JSON body".into());
    }

    let verdict = if details.is_empty() {
        Verdict::Match
    } else {
        Verdict::Diff
    };

    // Status-only mismatch without JSON field issues still counts.
    let critical = critical
        || (verdict == Verdict::Diff
            && go_status != rust_status
            && kotlin_critical_fields().contains_key(critical_key));

    RouteResult {
        route: route.to_string(),
        method: method.to_string(),
        go_status,
        rust_status,
        verdict,
        details,
        critical,
    }
}

fn diff_json(
    go: &Value,
    rust: &Value,
    path: &str,
    exact_keys: &[&str],
    id_type_only: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    match (go, rust) {
        (Value::Object(ga), Value::Object(ra)) => {
            let go_keys: BTreeSet<_> = ga.keys().cloned().collect();
            let rust_keys: BTreeSet<_> = ra.keys().cloned().collect();
            for k in go_keys.difference(&rust_keys) {
                let p = join_path(path, k);
                if ga.get(k).map(|v| v.is_null()).unwrap_or(false) {
                    out.push(format!("{p}: go null vs rust missing"));
                } else {
                    out.push(format!("{p}: present in go, missing in rust"));
                }
            }
            for k in rust_keys.difference(&go_keys) {
                let p = join_path(path, k);
                if ra.get(k).map(|v| v.is_null()).unwrap_or(false) {
                    out.push(format!("{p}: rust null vs go missing"));
                } else {
                    out.push(format!("{p}: present in rust, missing in go"));
                }
            }
            for k in go_keys.intersection(&rust_keys) {
                let p = join_path(path, k);
                let gv = &ga[k];
                let rv = &ra[k];
                // null vs missing already handled by key sets; null vs value:
                if gv.is_null() && !rv.is_null() {
                    out.push(format!(
                        "{p}: go null vs rust {rtype}",
                        rtype = type_name(rv)
                    ));
                    continue;
                }
                if rv.is_null() && !gv.is_null() {
                    out.push(format!(
                        "{p}: rust null vs go {gtype}",
                        gtype = type_name(gv)
                    ));
                    continue;
                }
                if type_name(gv) != type_name(rv) && !(gv.is_null() && rv.is_null()) {
                    out.push(format!(
                        "{p}: type go={} rust={}",
                        type_name(gv),
                        type_name(rv)
                    ));
                    continue;
                }
                let leaf = k.as_str();
                if id_type_only.contains(&leaf) {
                    // type already matches
                    continue;
                }
                if VOLATILE_KEYS.contains(&leaf) {
                    continue;
                }
                // /stats.path is each host's data dir — skip exact match at root only.
                if leaf == "path" && path.is_empty() {
                    continue;
                }
                let must_exact = exact_keys.contains(&leaf) || EXACT_VALUE_KEYS.contains(&leaf);
                if must_exact && gv != rv {
                    // Allow streamUrl host/port difference already excluded via VOLATILE.
                    out.push(format!(
                        "{p}: value go={} rust={}",
                        summarize(gv),
                        summarize(rv)
                    ));
                    continue;
                }
                out.extend(diff_json(gv, rv, &p, exact_keys, id_type_only));
            }
        }
        (Value::Array(ga), Value::Array(ra)) => {
            if ga.len() != ra.len() {
                out.push(format!(
                    "{path}: array length go={} rust={}",
                    ga.len(),
                    ra.len()
                ));
            }
            let n = ga.len().min(ra.len());
            for i in 0..n {
                let p = format!("{path}[{i}]");
                out.extend(diff_json(&ga[i], &ra[i], &p, exact_keys, id_type_only));
            }
        }
        (a, b) if a == b => {}
        (a, b) => {
            // scalar mismatch at this path — only report if exact
            let leaf = path
                .rsplit(['.', '[', ']'])
                .find(|s| !s.is_empty())
                .unwrap_or(path);
            if exact_keys.contains(&leaf) || EXACT_VALUE_KEYS.contains(&leaf) {
                out.push(format!(
                    "{path}: value go={} rust={}",
                    summarize(a),
                    summarize(b)
                ));
            } else if type_name(a) != type_name(b) {
                out.push(format!(
                    "{path}: type go={} rust={}",
                    type_name(a),
                    type_name(b)
                ));
            }
        }
    }
    out
}

fn join_path(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_string()
    } else {
        format!("{base}.{key}")
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "number/int"
            } else {
                "number/float"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn summarize(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 80 {
        format!("{}…", &s[..80])
    } else {
        s
    }
}

fn render_report(results: &[RouteResult], info_hash: &str) -> String {
    let mut out = String::new();
    out.push_str("# Control API Diff: Go vs Rust\n\n");
    out.push_str("Generated by `wt-parity api-diff`.\n\n");
    out.push_str(&format!("Fixture infoHash: `{info_hash}`\n\n"));

    let matches = results
        .iter()
        .filter(|r| r.verdict == Verdict::Match)
        .count();
    let diffs = results
        .iter()
        .filter(|r| r.verdict == Verdict::Diff)
        .count();
    let critical = results
        .iter()
        .filter(|r| r.verdict == Verdict::Diff && r.critical)
        .count();
    out.push_str(&format!(
        "**Summary:** {matches} MATCH · {diffs} DIFF · {critical} CRITICAL\n\n"
    ));

    out.push_str("## Kotlin-read fields (critical)\n\n");
    out.push_str(
        "Parsed by `android/core/.../EngineClient.kt` (consumed from `webtor/app/*.kt`):\n\n",
    );
    for (route, fields) in kotlin_critical_fields() {
        if route.contains("files[]") {
            continue;
        }
        let list: Vec<_> = fields.iter().copied().collect();
        out.push_str(&format!("- `{route}`: {}\n", list.join(", ")));
    }
    out.push('\n');

    out.push_str("## Routes\n\n");
    out.push_str("| Method | Route | Go | Rust | Verdict | Details |\n");
    out.push_str("|--------|-------|----|------|---------|--------|\n");
    for r in results {
        let crit = if r.critical && r.verdict == Verdict::Diff {
            " **CRITICAL**"
        } else {
            ""
        };
        let details = if r.details.is_empty() {
            String::new()
        } else {
            r.details.join("; ").replace('|', "\\|")
        };
        out.push_str(&format!(
            "| {} | {} | {:?} | {:?} | {}{} | {} |\n",
            r.method,
            r.route.replace('|', "\\|"),
            r.go_status,
            r.rust_status,
            r.verdict.as_str(),
            crit,
            details
        ));
    }
    out.push('\n');

    out.push_str("## Per-route detail\n\n");
    for r in results {
        out.push_str(&format!(
            "### {} {} — {}\n\n",
            r.method,
            r.route,
            r.verdict.as_str()
        ));
        out.push_str(&format!(
            "- HTTP: go={:?} rust={:?}\n",
            r.go_status, r.rust_status
        ));
        if r.critical {
            out.push_str("- **CRITICAL** (Kotlin-read field or status on a parsed route)\n");
        }
        if r.details.is_empty() {
            out.push_str("- no differences\n");
        } else {
            for d in &r.details {
                out.push_str(&format!("- {d}\n"));
            }
        }
        out.push('\n');
    }

    out.push_str("## Notes\n\n");
    out.push_str(
        "- Compared: HTTP status, JSON field names, types, null vs missing, array shapes,\n",
    );
    out.push_str(
        "  and exact values for ids/names/lengths/file lists (volatile speeds/ports skipped).\n",
    );
    out.push_str("- `id` from `/add` is type-checked only (UUID differs per engine instance).\n");
    out.push_str("- `streamUrl` host port differs per engine; treated as volatile.\n");
    out.push_str("- Rust engine may still be wiring core; many DIFFs are expected.\n");
    out
}

//! Port progress report: compares each Rust crate with the Go code it replaces
//! and writes docs/PROGRESS.md with ASCII bars.
//!
//! Build and run from engine-rs/:
//!   rustc -O tools/progress.rs -o /tmp/wt-progress && /tmp/wt-progress
//! Worker status comes from docs/status.txt ("<crate> <status> <note...>").

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const BAR: usize = 30;

struct Crate {
    name: &'static str,
    owner: &'static str,
    /// Go sources, relative to engine-go/. Entries ending in '/' are whole directories.
    go: &'static [&'static str],
}

const P: &str = "patches/torrent/";

fn crates() -> Vec<Crate> {
    vec![
        Crate { name: "wt-meta", owner: "grok-1", go: &["patches/torrent/bencode/", "patches/torrent/metainfo/", "patches/torrent/merkle/", "patches/torrent/types/infohash/", "patches/torrent/types/infohash-v2/"] },
        Crate { name: "wt-wire", owner: "agy-1", go: &["patches/torrent/peer_protocol/", "patches/torrent/mse/", "patches/torrent/handshake.go", "patches/torrent/ltep.go"] },
        Crate { name: "wt-discovery", owner: "grok-2", go: &["patches/torrent/tracker/", "patches/torrent/webtorrent/", "patches/torrent/webseed/", "patches/torrent/client-tracker-announcer.go", "patches/torrent/dht.go", "patches/torrent/bep40.go"] },
        Crate { name: "wt-core", owner: "codex-1+4", go: &["@core"] },
        Crate { name: "wt-storage", owner: "agy-3, grok-5, grok-7", go: &["storage.go", "direct_storage.go", "cache_store.go", "cache_lru.go", "checkpoint.go", "extent.go", "extent_host.go", "pieces.go", "pieces_host.go", "stream_ranges_host.go", "sparse_unix.go", "sparse_other.go", "verification.go", "restore_hash.go", "zero_hash.go"] },
        Crate { name: "wt-policy", owner: "grok-3", go: &["adaptive_buffer.go", "arrival_model.go", "video_priority.go", "playback_grid.go", "playback_resume.go", "next_preload.go", "peer_exploration.go", "seek_log.go", "ghost_mode.go", "ghost_pro.go", "bypass.go", "transfer_rates.go", "direct_play_idle.go", "claude_seek.go"] },
        Crate { name: "wt-parity", owner: "grok-1, codex-3", go: &[] },
        Crate { name: "wt-engine", owner: "codex-2+5", go: &["main.go", "main_host.go", "server.go", "types.go", "playback.go", "preview.go", "live_preview.go", "trackers.go", "tracker_status.go", "network.go", "connections.go", "seeding.go", "version.go"] },
    ]
}

/// Top-level patches/torrent files that belong to other crates.
const NOT_CORE: &[&str] = &["handshake.go", "ltep.go", "client-tracker-announcer.go", "dht.go", "bep40.go"];

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == "testdata") {
                continue;
            }
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

fn go_files(go_root: &Path, c: &Crate) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for g in c.go {
        if *g == "@core" {
            let dir = go_root.join(P);
            for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                if p.is_file() && n.ends_with(".go") && !NOT_CORE.contains(&n.as_str()) {
                    out.push(p);
                }
            }
            walk(&dir.join("request-strategy"), "go", &mut out);
            walk(&dir.join("internal/request-strategy"), "go", &mut out);
        } else if g.ends_with('/') {
            walk(&go_root.join(g), "go", &mut out);
        } else {
            let p = go_root.join(g);
            out.push(p.clone());
            // Each Go file's own tests count toward the test target.
            let t = p.with_file_name(format!("{}_test.go", g.trim_end_matches(".go").rsplit('/').next().unwrap()));
            if t.exists() {
                out.push(t);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[derive(Default)]
struct Count {
    code_lines: usize,
    tests: usize,
    stubs: usize,
}

fn count(files: &[PathBuf], go: bool) -> Count {
    let mut c = Count::default();
    for f in files {
        let Ok(text) = fs::read_to_string(f) else { continue };
        let is_test_file = f.to_string_lossy().ends_with("_test.go") || f.components().any(|x| x.as_os_str() == "tests");
        let mut in_rust_tests = false;
        for line in text.lines() {
            let t = line.trim();
            if go {
                if t.starts_with("func Test") {
                    c.tests += 1;
                }
            } else {
                if t == "#[test]" || t.starts_with("#[tokio::test") {
                    c.tests += 1;
                }
                if t.starts_with("#[cfg(test)]") {
                    in_rust_tests = true;
                }
                c.stubs += t.matches("unimplemented!(").count() + t.matches("todo!(").count();
            }
            let code = !t.is_empty() && !t.starts_with("//");
            if code && !is_test_file && !in_rust_tests {
                c.code_lines += 1;
            }
        }
    }
    c
}

/// Hours after the crates are done for integration, Go-vs-Rust benchmarks and
/// the Android build. A planning guess, not measured.
const AFTER_CRATES_HOURS: f64 = 4.0;

fn clock(unix: u64) -> String {
    std::process::Command::new("date")
        .args(["-d", &format!("@{unix}"), "+%H:%M"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| unix.to_string())
}

/// Records (time, overall) in docs/progress_history.txt and projects the
/// finish from the rate over the last hour (or since the start).
fn estimate(root: &Path, overall: f64, pending: &[(String, f64)]) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let path = root.join("docs/progress_history.txt");
    let mut hist: Vec<(u64, f64)> = fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (t, p) = l.split_once(' ')?;
            Some((t.parse().ok()?, p.parse().ok()?))
        })
        .collect();
    hist.push((now, overall));
    let text: String = hist.iter().map(|(t, p)| format!("{t} {p:.4}\n")).collect();
    let _ = fs::write(&path, text);

    let start = hist[0];
    let base = hist.iter().find(|(t, _)| now.saturating_sub(*t) <= 3600).copied().unwrap_or(start);
    let base = if now.saturating_sub(base.0) < 600 { start } else { base };
    let elapsed_h = now.saturating_sub(start.0) as f64 / 3600.0;
    let mut out = format!(
        "Started {} · elapsed {:.1} h · now {:.0}%\n\n",
        clock(start.0),
        elapsed_h,
        overall * 100.0
    );
    let span = now.saturating_sub(base.0) as f64;
    let gained = overall - base.1;
    if span < 300.0 || gained <= 0.0 {
        out.push_str("ETA: not enough history yet to estimate.\n");
        return out;
    }
    let rate_per_h = gained / span * 3600.0;
    let mut crates_h = (1.0 - overall) / rate_per_h;
    // Critical path: the slowest pending crate at its own recent rate.
    let cpath = root.join("docs/progress_history_crates.txt");
    let mut lines = fs::read_to_string(&cpath).unwrap_or_default();
    for (name, pct) in pending {
        lines.push_str(&format!("{now} {name} {pct:.4}\n"));
    }
    let _ = fs::write(&cpath, &lines);
    let mut critical = String::new();
    for (name, pct) in pending {
        let first = lines.lines().filter_map(|l| {
            let mut it = l.split(' ');
            let t: u64 = it.next()?.parse().ok()?;
            (it.next()? == name).then_some(())?;
            Some((t, it.next()?.parse::<f64>().ok()?))
        }).find(|(t, _)| now.saturating_sub(*t) <= 3600);
        if let Some((t, p0)) = first {
            let dt = now.saturating_sub(t) as f64;
            if dt >= 300.0 && pct > &p0 {
                let h = (1.0 - pct) / ((pct - p0) / dt * 3600.0);
                if h > crates_h {
                    crates_h = h;
                    critical = name.clone();
                }
            }
        }
    }
    if crates_h.is_finite() && !critical.is_empty() {
        out.push_str(&format!("Critical path: {critical}\n\n"));
    }
    let crates_at = now + (crates_h * 3600.0) as u64;
    let full_at = crates_at + (AFTER_CRATES_HOURS * 3600.0) as u64;
    out.push_str(&format!(
        "ETA (rough, from the last {:.0} min at {:.1}%/h):\n\n- crates complete: ~{} (in {:.1} h)\n- whole port incl. integration, benchmarks vs Go, Android APK: ~{} (in {:.1} h; the {:.0} h after the crates is a planning guess)\n",
        span / 60.0,
        rate_per_h * 100.0,
        clock(crates_at),
        crates_h,
        clock(full_at),
        crates_h + AFTER_CRATES_HOURS,
        AFTER_CRATES_HOURS
    ));
    out
}

fn bar(frac: f64) -> String {
    let filled = ((frac.clamp(0.0, 1.0)) * BAR as f64).round() as usize;
    format!("[{}{}]", "#".repeat(filled), ".".repeat(BAR - filled))
}

fn main() {
    let root = std::env::current_dir().unwrap(); // engine-rs/
    let go_root = root.join("../engine-go");
    let status: HashMap<String, (String, String)> = fs::read_to_string(root.join("docs/status.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, ' ');
            Some((it.next()?.to_string(), (it.next()?.to_string(), it.next().unwrap_or("").to_string())))
        })
        .collect();

    let mut rows = Vec::new();
    let (mut sum_done, mut sum_all) = (0.0, 0.0);
    for c in crates() {
        let go = count(&go_files(&go_root, &c), true);
        let mut rs_files = Vec::new();
        walk(&root.join("crates").join(c.name), "rs", &mut rs_files);
        let rs = count(&rs_files, false);
        let (st, note) = status.get(c.name).cloned().unwrap_or(("running".into(), String::new()));

        // Code: Rust lines against Go lines (Rust ports run about 0.8x Go).
        if c.go.is_empty() {
            // New harness crate, no Go counterpart: progress by status only.
            let pct = if st == "done" { 1.0 } else if rs.code_lines > 50 { 0.5 } else { 0.0 };
            rows.push((c, go, rs, pct, st, note));
            continue;
        }
        let code = (rs.code_lines as f64 / (go.code_lines as f64 * 0.8).max(1.0)).min(1.0);
        let tests = (rs.tests as f64 / (go.tests as f64).max(1.0)).min(1.0);
        let stub_penalty = if rs.stubs > 0 { 0.9 } else { 1.0 };
        let mut pct = ((0.6 * code + 0.4 * tests) * stub_penalty).min(0.95);
        if st == "done" {
            pct = 1.0;
        }
        sum_done += pct * go.code_lines as f64;
        sum_all += go.code_lines as f64;
        rows.push((c, go, rs, pct, st, note));
    }

    let overall = sum_done / sum_all.max(1.0);
    let done = rows.iter().filter(|r| r.4 == "done").count();
    let slow: Vec<(String, f64)> = rows.iter().filter(|r| r.4 != "done").map(|r| (r.0.name.to_string(), r.3)).collect();
    let eta = estimate(&root, overall, &slow);
    let mut md = String::new();
    md.push_str("# Rust engine port progress\n\n");
    md.push_str("Generated by `engine-rs/tools/progress.rs`. Estimates from code and test\n");
    md.push_str("counts; a crate is 100% only when its worker reports done and the\n");
    md.push_str("coordinator has verified it.\n\n```text\n");
    md.push_str(&format!("OVERALL   {} {:>3.0}%   crates complete {}/{}, pending {}\n\n", bar(overall), overall * 100.0, done, rows.len(), rows.len() - done));
    for (c, _, _, pct, st, _) in &rows {
        md.push_str(&format!("{:<13} {} {:>3.0}%  {:<8} {}\n", c.name, bar(*pct), pct * 100.0, st, c.owner));
    }
    md.push_str("```\n\n");
    md.push_str(&eta);
    md.push_str("\n| Crate | Owner | Status | Go lines | Rust lines | Go tests | Rust tests | Stubs left | Note |\n|---|---|---|---:|---:|---:|---:|---:|---|\n");
    for (c, go, rs, _, st, note) in &rows {
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n", c.name, c.owner, st, go.code_lines, rs.code_lines, go.tests, rs.tests, rs.stubs, note));
    }
    md.push_str("\n## Complete\n\n");
    let complete: Vec<_> = rows.iter().filter(|r| r.4 == "done").collect();
    if complete.is_empty() {
        md.push_str("- none yet\n");
    }
    for r in complete {
        md.push_str(&format!("- [x] {} ({})\n", r.0.name, r.0.owner));
    }
    md.push_str("\n## Pending\n\n");
    for r in rows.iter().filter(|r| r.4 != "done") {
        md.push_str(&format!("- [ ] {} ({}): {:.0}%, {} of ~{} Rust lines, {}/{} tests\n", r.0.name, r.0.owner, r.3 * 100.0, r.2.code_lines, (r.1.code_lines as f64 * 0.8) as usize, r.2.tests, r.1.tests));
    }
    md.push_str("- [ ] Integration: Rust engine end-to-end, parity tests vs Go\n");
    md.push_str("- [ ] Benchmarks: throughput, seek-to-first-byte, RAM vs Go\n");
    md.push_str("- [ ] Android: libengine.so (arm64) + APK build\n");
    let roster = fs::read_to_string(root.join("docs/roster.txt")).unwrap_or_default();
    if !roster.is_empty() {
        md.push_str("\n## Workers\n\n| Worker | Crate | Work | State |\n|---|---|---|---|\n");
        for l in roster.lines().filter(|l| !l.trim().is_empty()) {
            let f: Vec<&str> = l.split('|').collect();
            if f.len() == 4 {
                md.push_str(&format!("| {} | {} | {} | {} |\n", f[0], f[1], f[2], f[3]));
            }
        }
    }
    let events = fs::read_to_string(root.join("docs/events.txt")).unwrap_or_default();
    if !events.is_empty() {
        md.push_str("\n## Coordinator log\n\n");
        for l in events.lines().filter(|l| !l.trim().is_empty()) {
            let (t, e) = l.split_once('|').unwrap_or(("-", l));
            if t == "-" {
                md.push_str(&format!("- {e}\n"));
            } else {
                md.push_str(&format!("- {t} {e}\n"));
            }
        }
    }
    let log = std::process::Command::new("git").args(["log", "--oneline", "8d10638^..HEAD", "--", "."]).output();
    if let Ok(o) = log {
        md.push_str("\n## Commits\n\n```text\n");
        md.push_str(&String::from_utf8_lossy(&o.stdout));
        md.push_str("```\n");
    }
    fs::write(root.join("docs/PROGRESS.md"), &md).unwrap();
    print!("{md}");
}

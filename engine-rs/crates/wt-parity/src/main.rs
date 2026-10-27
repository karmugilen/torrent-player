//! wt-parity: Go vs Rust engine host parity + performance harness.

mod api_diff;
mod client;
mod compare;
mod engine;
mod fixture;
mod procstat;
mod proxy;
mod report;
mod scenarios;
mod swarm;

use api_diff::{run_api_diff, ApiDiffOpts};
use clap::{Parser, Subcommand};
use compare::{compare_files, print_table, DEFAULT_NOISE};
use scenarios::{run, RunOpts};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "wt-parity",
    about = "Parity/perf harness for engine host binaries"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run one or more scenarios against an engine host binary.
    Run {
        /// Path to engine host binary (`engine-go-host` or `engine-host`).
        #[arg(long)]
        engine: PathBuf,
        /// Label written into the result JSON (e.g. go, rust).
        #[arg(long)]
        label: String,
        /// Scenario name, or `all`.
        #[arg(long, default_value = "all")]
        scenario: String,
        /// Output JSON path.
        #[arg(long)]
        out: PathBuf,
        /// Working directory for fixtures/swarm/leech dirs.
        #[arg(long, default_value = "/home/kar/.cache/wt-tmp/wt-parity")]
        work: PathBuf,
        /// Repeats per scenario (>= 3 recommended).
        #[arg(long, default_value_t = 3)]
        repeats: u32,
        /// Number of local seeders.
        #[arg(long, default_value_t = 4)]
        seeders: usize,
    },
    /// Compare two result JSON files; exit 1 on FAIL.
    Compare {
        go_json: PathBuf,
        rust_json: PathBuf,
        /// Allowed fraction Rust may be worse (default 0.05).
        #[arg(long, default_value_t = DEFAULT_NOISE)]
        noise: f64,
    },
    /// Diff Go vs Rust control API responses (HTTP status + JSON contract).
    ApiDiff {
        /// Path to Go `engine-go-host` binary (built if missing / with `--build`).
        #[arg(long)]
        go: PathBuf,
        /// Path to Rust `engine-host` binary (built if missing / with `--build`).
        #[arg(long)]
        rust: PathBuf,
        /// Work/fixtures dir (localhost only; cleaned after run).
        #[arg(long, default_value = "/home/kar/.cache/wt-tmp/api-diff")]
        work: PathBuf,
        /// Markdown report path.
        #[arg(long, default_value = "crates/wt-parity/API_DIFF.md")]
        report: PathBuf,
        /// Repo root containing `engine-go/` and `engine-rs/` (for builds).
        #[arg(long, default_value = "..")]
        repo: PathBuf,
        /// Force rebuild of both host binaries before the run.
        #[arg(long, default_value_t = false)]
        build: bool,
    },
}

fn main() -> ExitCode {
    let running = Arc::new(AtomicBool::new(true));
    {
        let r = Arc::clone(&running);
        let _ = ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
            eprintln!("wt-parity: interrupt — child cleanup via Drop");
        });
    }
    let _ = running.load(Ordering::SeqCst);

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run {
            engine,
            label,
            scenario,
            out,
            work,
            repeats,
            seeders,
        } => {
            if !engine.exists() {
                eprintln!("engine binary not found: {}", engine.display());
                return ExitCode::from(2);
            }
            if let Some(parent) = out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::create_dir_all(&work);
            match run(RunOpts {
                engine,
                label,
                scenario,
                out: out.clone(),
                work,
                repeats,
                seeders,
            }) {
                Ok(report) => match serde_json::to_string_pretty(&report) {
                    Ok(s) => {
                        if let Err(e) = std::fs::write(&out, s) {
                            eprintln!("write {}: {e}", out.display());
                            return ExitCode::from(1);
                        }
                        eprintln!("wt-parity: wrote {}", out.display());
                        for (k, m) in &report.metrics {
                            eprintln!(
                                "  {k}: median={:.3} {} (n={})",
                                m.median,
                                m.unit,
                                m.samples.len()
                            );
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("serialize: {e}");
                        ExitCode::from(1)
                    }
                },
                Err(e) => {
                    eprintln!("wt-parity run failed: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Cmd::Compare {
            go_json,
            rust_json,
            noise,
        } => match compare_files(&go_json, &rust_json, noise) {
            Ok(report) => {
                print_table(&report);
                if report.pass {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(e) => {
                eprintln!("compare failed: {e}");
                ExitCode::from(2)
            }
        },
        Cmd::ApiDiff {
            go,
            rust,
            work,
            report,
            repo,
            build,
        } => {
            let repo = if repo.is_relative() {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(repo)
            } else {
                repo
            };
            // Resolve report relative to engine-rs when run from there.
            let report = if report.is_relative() {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(report)
            } else {
                report
            };
            match run_api_diff(ApiDiffOpts {
                go_bin: go,
                rust_bin: rust,
                work,
                report,
                repo,
                build,
            }) {
                Ok(summary) => {
                    eprintln!(
                        "api-diff: {} MATCH, {} DIFF, {} CRITICAL",
                        summary.match_count,
                        summary.diff_count,
                        summary.critical_diffs.len()
                    );
                    for c in &summary.critical_diffs {
                        eprintln!("  CRITICAL: {c}");
                    }
                    eprintln!("api-diff: report {}", summary.report_path.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("api-diff failed: {e}");
                    ExitCode::from(1)
                }
            }
        }
    }
}

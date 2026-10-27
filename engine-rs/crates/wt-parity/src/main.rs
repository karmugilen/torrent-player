//! wt-parity: Go vs Rust engine host parity + performance harness.

mod client;
mod compare;
mod engine;
mod fixture;
mod procstat;
mod proxy;
mod report;
mod scenarios;
mod swarm;

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
    }
}

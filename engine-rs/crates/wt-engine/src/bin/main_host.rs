//! Desktop/host control binary — port of `engine-go/main_host.go`.
//!
//! Flags match Go (`-port`, `-path`, `-max-peers`) and also accept the GNU
//! long form (`--port`, `--path`, `--max-peers`). Until agy-4 exposes a sync
//! host entrypoint from `lib.rs`, this binary parses flags, waits for
//! SIGINT/SIGTERM, and prints "engine not wired yet" so the crate compiles.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const DEFAULT_PORT: u16 = 18080;
const DEFAULT_PATH: &str = "/tmp/webtorrent";
const DEFAULT_MAX_PEERS: usize = 55;

struct HostArgs {
    port: u16,
    path: String,
    max_peers: usize,
}

fn print_usage(argv0: &str) {
    eprintln!("Usage: {argv0} [-port|--port PORT] [-path|--path DIR] [-max-peers|--max-peers N]");
    eprintln!("  -port       control server port (default {DEFAULT_PORT})");
    eprintln!("  -path       download directory (default {DEFAULT_PATH})");
    eprintln!("  -max-peers  maximum peers (default {DEFAULT_MAX_PEERS})");
}

fn parse_args(args: &[String]) -> Result<HostArgs, String> {
    let mut port = DEFAULT_PORT;
    let mut path = DEFAULT_PATH.to_string();
    let mut max_peers = DEFAULT_MAX_PEERS;

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        let (key, inline_val) = if let Some(eq) = arg.find('=') {
            (&arg[..eq], Some(&arg[eq + 1..]))
        } else {
            (arg.as_str(), None)
        };

        let normalized = match key {
            "-h" | "--help" | "-help" => {
                print_usage(&args[0]);
                std::process::exit(0);
            }
            "-port" | "--port" => "port",
            "-path" | "--path" => "path",
            "-max-peers" | "--max-peers" => "max-peers",
            other if other.starts_with('-') => {
                return Err(format!("unknown flag: {other}"));
            }
            other => {
                return Err(format!("unexpected argument: {other}"));
            }
        };

        let value = if let Some(v) = inline_val {
            v.to_string()
        } else {
            i += 1;
            if i >= args.len() {
                return Err(format!("flag {key} requires a value"));
            }
            args[i].clone()
        };

        match normalized {
            "port" => {
                port = value
                    .parse::<u16>()
                    .map_err(|_| format!("invalid port: {value}"))?;
            }
            "path" => path = value,
            "max-peers" => {
                max_peers = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid max-peers: {value}"))?;
            }
            _ => unreachable!(),
        }
        i += 1;
    }

    Ok(HostArgs {
        port,
        path,
        max_peers,
    })
}

fn install_stop_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let flag_int = flag.clone();
    let flag_term = flag.clone();

    // tokio signal handlers on a background runtime thread.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return,
        };
        rt.block_on(async move {
            let sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
            let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
            let (mut sigint, mut sigterm) = match (sigint, sigterm) {
                (Ok(i), Ok(t)) => (i, t),
                _ => return,
            };
            tokio::select! {
                _ = sigint.recv() => flag_int.store(true, Ordering::SeqCst),
                _ = sigterm.recv() => flag_term.store(true, Ordering::SeqCst),
            }
        });
    });

    flag
}

fn run_engine(args: HostArgs) -> i32 {
    let stop = install_stop_flag();

    if let Some(code) = try_run_wired_engine(&args, &stop) {
        return code;
    }

    eprintln!(
        "engine not wired yet (port={}, path={}, max_peers={})",
        args.port, args.path, args.max_peers
    );
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    0
}

/// Hook for agy-4's `EngineServer`. Returns `None` while unwired.
fn try_run_wired_engine(_args: &HostArgs, _stop: &AtomicBool) -> Option<i32> {
    // When lib.rs exports `wt_engine::start_host_server` (or similar), call it
    // here and return Some(exit_code). Until then the stub path runs.
    None
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(err) => {
            eprintln!("{err}");
            print_usage(argv.first().map(String::as_str).unwrap_or("engine-host"));
            return ExitCode::from(2);
        }
    };
    ExitCode::from(run_engine(args) as u8)
}

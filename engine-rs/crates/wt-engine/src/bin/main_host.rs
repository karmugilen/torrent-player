//! Desktop/host control binary — port of `engine-go/main_host.go`.
//!
//! Flags match Go (`-port`, `-path`, `-max-peers`) and also accept the GNU
//! long form (`--port`, `--path`, `--max-peers`). Starts `EngineServer`, the
//! media stream listener, and a localhost control HTTP server
//! ([`engine::ctl_http`]), then waits for SIGINT/SIGTERM / Ctrl-C, stdin EOF
//! (when `ENGINE_EXIT_ON_STDIN_EOF=1`), or `POST /shutdown`.

use std::io::Read;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use engine::ctl_http;
use engine::server::EngineServer;

const DEFAULT_PORT: u16 = 18080;
const DEFAULT_PATH: &str = "/tmp/webtorrent";
const DEFAULT_MAX_PEERS: usize = 55;

struct HostArgs {
    port: u16,
    path: String,
    max_peers: usize,
    half_open_conns: Option<usize>,
    no_port_forwarding: bool,
}

fn print_usage(argv0: &str) {
    eprintln!("Usage: {argv0} [-port|--port PORT] [-path|--path DIR] [-max-peers|--max-peers N] [--half-open-conns N]");
    eprintln!("  -port       control server port (default {DEFAULT_PORT})");
    eprintln!("  -path       download directory (default {DEFAULT_PATH})");
    eprintln!("  -max-peers  maximum peers (default {DEFAULT_MAX_PEERS})");
    eprintln!("  --half-open-conns  override pending dials per torrent and globally");
    eprintln!("  --no-port-forwarding  no router port mapping or local peer discovery");
}

fn parse_args(args: &[String]) -> Result<HostArgs, String> {
    let mut port = DEFAULT_PORT;
    let mut path = DEFAULT_PATH.to_string();
    let mut max_peers = DEFAULT_MAX_PEERS;
    let mut half_open_conns = None;
    let mut no_port_forwarding = false;

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
            "--half-open-conns" => "half-open-conns",
            "--no-port-forwarding" => {
                no_port_forwarding = true;
                i += 1;
                continue;
            }
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
            "half-open-conns" => {
                let limit = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid half-open-conns: {value}"))?;
                if limit == 0 {
                    return Err("half-open-conns must be positive".into());
                }
                half_open_conns = Some(limit);
            }
            _ => unreachable!(),
        }
        i += 1;
    }

    Ok(HostArgs {
        port,
        path,
        max_peers,
        half_open_conns,
        no_port_forwarding,
    })
}

fn env_flag_true(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}

/// Install stop flag watchers: Ctrl-C (all platforms), SIGTERM (Unix), and
/// optional stdin-EOF when `ENGINE_EXIT_ON_STDIN_EOF=1`.
fn install_stop_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));

    {
        let flag_ctrl = flag.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                #[cfg(unix)]
                {
                    let sigint =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
                    let sigterm =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
                    match (sigint, sigterm) {
                        (Ok(mut sigint), Ok(mut sigterm)) => {
                            tokio::select! {
                                _ = sigint.recv() => flag_ctrl.store(true, Ordering::SeqCst),
                                _ = sigterm.recv() => flag_ctrl.store(true, Ordering::SeqCst),
                            }
                        }
                        _ => {
                            let _ = tokio::signal::ctrl_c().await;
                            flag_ctrl.store(true, Ordering::SeqCst);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = tokio::signal::ctrl_c().await;
                    flag_ctrl.store(true, Ordering::SeqCst);
                }
            });
        });
    }

    if env_flag_true("ENGINE_EXIT_ON_STDIN_EOF") {
        let flag_stdin = flag.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 256];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => {
                        // EOF: same clean shutdown as SIGTERM.
                        flag_stdin.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(_) => continue,
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        flag_stdin.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        });
    }

    flag
}

fn run_engine(args: HostArgs) -> i32 {
    let stop = install_stop_flag();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("engine-host")
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to create tokio runtime: {err}");
            return 1;
        }
    };

    rt.block_on(async move {
        let server =
            match EngineServer::new_with_config(args.port, &args.path, args.max_peers, |config| {
                if let Some(limit) = args.half_open_conns {
                    config.half_open_conns_per_torrent = limit;
                    config.total_half_open_conns = limit;
                }
                if args.no_port_forwarding {
                    // Local benchmarks: leave the router and the LAN alone.
                    config.port_forwarding = false;
                    config.local_service_discovery = false;
                }
            }) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("{err}");
                    return 1;
                }
            };

        if let Err(err) = server.start_streaming().await {
            eprintln!("{err}");
            server.close();
            return 1;
        }

        let ctl = match ctl_http::serve(server.clone(), args.port).await {
            Ok(ctl) => ctl,
            Err(err) => {
                eprintln!("control server: {err}");
                server.close();
                return 1;
            }
        };

        let done_rx = ctl.take_done();
        match done_rx {
            Some(mut done_rx) => loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                tokio::select! {
                    _ = &mut done_rx => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
                }
            },
            None => {
                while !stop.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }

        ctl.stop();
        server.close();
        0
    })
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

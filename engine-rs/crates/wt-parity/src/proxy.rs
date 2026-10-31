//! Tiny TCP proxy with one-way inbound delay and upload rate limit.
//!
//! Mirrors seek_search_test.go's latencyConn idea: delay applies to data the
//! leech *reads* (seeder → leech), so it adds once per RTT, not twice.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub struct ProxyHandle {
    pub listen_port: u16,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl ProxyHandle {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake accept.
        let _ = TcpStream::connect(("127.0.0.1", self.listen_port));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.listen_port));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Start a proxy: clients connect to the returned port; bytes are forwarded to
/// `upstream_host:upstream_port` with `delay` on the upstream→client path and
/// `bytes_per_sec` rate limit on that same path (0 = unlimited).
pub fn start_proxy(
    upstream_host: &str,
    upstream_port: u16,
    delay: Duration,
    bytes_per_sec: u64,
) -> Result<ProxyHandle, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener.set_nonblocking(false).ok();
    let listen_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = Arc::clone(&stop);
    let upstream = format!("{upstream_host}:{upstream_port}");

    let join = thread::spawn(move || {
        for conn in listener.incoming() {
            if stop_c.load(Ordering::SeqCst) {
                break;
            }
            let Ok(client) = conn else { continue };
            let upstream = upstream.clone();
            let stop_c = Arc::clone(&stop_c);
            thread::spawn(move || {
                if let Err(e) = handle_conn(client, &upstream, delay, bytes_per_sec, stop_c) {
                    eprintln!("wt-parity proxy: {e}");
                }
            });
        }
    });

    Ok(ProxyHandle {
        listen_port,
        stop,
        join: Some(join),
    })
}

fn handle_conn(
    client: TcpStream,
    upstream: &str,
    delay: Duration,
    bytes_per_sec: u64,
    global_stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let server = TcpStream::connect(upstream).map_err(|e| format!("dial {upstream}: {e}"))?;
    client.set_nodelay(true).ok();
    server.set_nodelay(true).ok();

    let conn_stop = Arc::new(AtomicBool::new(false));

    let mut client_rd = client.try_clone().map_err(|e| e.to_string())?;
    let mut client_wr = client.try_clone().map_err(|e| e.to_string())?;
    let mut server_rd = server.try_clone().map_err(|e| e.to_string())?;
    let mut server_wr = server;

    let cs1 = Arc::clone(&conn_stop);
    let gs1 = Arc::clone(&global_stop);
    let s_rd_clone = server_rd.try_clone().ok();
    let c_rd_clone = client_rd.try_clone().ok();

    let t1 = thread::spawn(move || {
        let mut buf = [0u8; 32 * 1024];
        while !cs1.load(Ordering::SeqCst) && !gs1.load(Ordering::SeqCst) {
            match client_rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if server_wr.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        cs1.store(true, Ordering::SeqCst);
        let _ = server_wr.shutdown(std::net::Shutdown::Both);
        if let Some(s) = s_rd_clone {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    });

    let cs2 = Arc::clone(&conn_stop);
    let gs2 = global_stop;

    let t2 = thread::spawn(move || {
        // A delay line: sleeping the delay per read would cap the link at one
        // read per delay, whatever its configured rate.
        let (tx, rx) = std::sync::mpsc::sync_channel::<(Instant, Vec<u8>)>(64);
        let writer = thread::spawn(move || {
            let mut bucket = RateBucket::new(bytes_per_sec);
            while let Ok((due, data)) = rx.recv() {
                let now = Instant::now();
                if due > now {
                    thread::sleep(due - now);
                }
                bucket.wait_for(data.len() as u64);
                if client_wr.write_all(&data).is_err() {
                    break;
                }
            }
            let _ = client_wr.shutdown(std::net::Shutdown::Both);
        });
        let mut buf = [0u8; 32 * 1024];
        while !cs2.load(Ordering::SeqCst) && !gs2.load(Ordering::SeqCst) {
            match server_rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send((Instant::now() + delay, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        drop(tx);
        let _ = writer.join();
        cs2.store(true, Ordering::SeqCst);
        if let Some(c) = c_rd_clone {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    });

    let _ = t1.join();
    let _ = t2.join();
    Ok(())
}

struct RateBucket {
    bytes_per_sec: u64,
    tokens: f64,
    last: Instant,
}

impl RateBucket {
    fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec,
            tokens: bytes_per_sec as f64,
            last: Instant::now(),
        }
    }

    fn wait_for(&mut self, n: u64) {
        if self.bytes_per_sec == 0 {
            return;
        }
        loop {
            let now = Instant::now();
            let elapsed = now.duration_since(self.last).as_secs_f64();
            self.last = now;
            self.tokens = (self.tokens + elapsed * self.bytes_per_sec as f64)
                .min(self.bytes_per_sec as f64 * 2.0);
            if self.tokens >= n as f64 {
                self.tokens -= n as f64;
                return;
            }
            let need = n as f64 - self.tokens;
            let sleep_s = need / self.bytes_per_sec as f64;
            thread::sleep(Duration::from_secs_f64(sleep_s.max(0.001)));
        }
    }
}

/// Four speeds from seek_search_test.go: 40KiB, 150KiB, 400KiB, 1MiB /s.
pub const SPEEDS: [u64; 4] = [40 << 10, 150 << 10, 400 << 10, 1 << 20];

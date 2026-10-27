//! Sample RSS / CPU of a child engine via /proc.

use std::fs;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct ProcSample {
    pub peak_rss_bytes: u64,
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
}

pub struct ProcMonitor {
    pid: u32,
    start: Instant,
    start_cpu: f64,
    peak_rss: u64,
}

impl ProcMonitor {
    pub fn start(pid: u32) -> Self {
        let start_cpu = cpu_seconds(pid).unwrap_or(0.0);
        let peak_rss = rss_bytes(pid).unwrap_or(0);
        Self {
            pid,
            start: Instant::now(),
            start_cpu,
            peak_rss,
        }
    }

    pub fn tick(&mut self) {
        if let Some(rss) = rss_bytes(self.pid) {
            self.peak_rss = self.peak_rss.max(rss);
        }
    }

    pub fn finish(mut self) -> ProcSample {
        self.tick();
        let end_cpu = cpu_seconds(self.pid).unwrap_or(self.start_cpu);
        ProcSample {
            peak_rss_bytes: self.peak_rss,
            cpu_seconds: (end_cpu - self.start_cpu).max(0.0),
            wall_seconds: self.start.elapsed().as_secs_f64(),
        }
    }
}

pub fn rss_bytes(pid: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

pub fn cpu_seconds(pid: u32) -> Option<f64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm can contain spaces/parens; find last ')' then fields after.
    let after = stat.rsplit(')').next()?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    // After ')': state is fields[0]; utime=fields[11], stime=fields[12] (0-based from after ')').
    // /proc/pid/stat: utime is field 14, stime field 15 (1-based whole). After ') ' the first
    // token is field 3 (state), so utime is index 11, stime index 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let ticks = utime + stime;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    if hz <= 0.0 {
        return None;
    }
    Some(ticks as f64 / hz)
}

/// Poll RSS in a background thread until the channel closes.
pub fn spawn_rss_poller(
    pid: u32,
    every: Duration,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<u64>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let mut peak = rss_bytes(pid).unwrap_or(0);
        loop {
            match rx.recv_timeout(every) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(rss) = rss_bytes(pid) {
                        peak = peak.max(rss);
                    }
                }
            }
        }
        if let Some(rss) = rss_bytes(pid) {
            peak = peak.max(rss);
        }
        peak
    });
    (tx, handle)
}

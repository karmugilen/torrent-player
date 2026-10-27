//! Seek latency CSV logging helpers (`engine-go/seek_log.go`).
//!
//! No file I/O here: the engine formats lines with [`SeekLog::format_line`] and
//! decides rotation with [`SeekLog::should_rotate`]. [`StreamMeter`] times
//! bytes handed to the player using an injected clock.

use std::time::{Duration, Instant};

pub const SEEK_LOG_MAX_BYTES: u64 = 2 << 20;
pub const SEEK_LOG_HEADER: &str = "time,build,torrent,file,kind,offset_mib,position_pct,first_ms,mib1_ms,mib2_ms,total_ms,bytes,in_ram,peers,unchoked_us,down_kib_s,variant,rescues,mode,piece_kib\n";

/// One seek / range-request measurement.
#[derive(Clone, Debug)]
pub struct SeekRecord {
    pub at: String,
    pub torrent: String,
    pub file: i32,
    pub kind: String,
    pub offset: i64,
    pub length: i64,
    pub first: Duration,
    pub m1: Duration,
    pub m2: Duration,
    pub total: Duration,
    pub bytes: i64,
    pub in_ram: bool,
    pub peers: i32,
    pub unchoked: i32,
    pub down_bytes_sec: i64,
    pub variant: String,
    pub rescues: i64,
    pub direct_play: bool,
    pub piece_bytes: i64,
}

/// Build-tag + path metadata; formatting only.
#[derive(Clone, Debug, Default)]
pub struct SeekLog {
    pub tag: String,
}

impl SeekLog {
    pub fn new(tag: impl Into<String>) -> Self {
        Self { tag: tag.into() }
    }

    pub fn header() -> &'static str {
        SEEK_LOG_HEADER
    }

    pub fn should_rotate(file_size: u64) -> bool {
        file_size > SEEK_LOG_MAX_BYTES
    }

    /// True when an existing file's first line is not the current header.
    pub fn header_mismatch(existing_header: &str) -> bool {
        existing_header != SEEK_LOG_HEADER
    }

    pub fn format_line(&self, r: &SeekRecord) -> String {
        let ms = |d: Duration| {
            if d.is_zero() {
                String::new()
            } else {
                d.as_millis().to_string()
            }
        };
        let pct = if r.length > 0 {
            r.offset as f64 * 100.0 / r.length as f64
        } else {
            0.0
        };
        let in_ram = if r.in_ram { 1 } else { 0 };
        let mode = if r.direct_play { "direct" } else { "download" };
        format!(
            "{},{},{},{},{},{:.1},{:.1},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            r.at,
            self.tag,
            r.torrent,
            r.file,
            r.kind,
            r.offset as f64 / (1 << 20) as f64,
            pct,
            ms(r.first),
            ms(r.m1),
            ms(r.m2),
            r.total.as_millis(),
            r.bytes,
            in_ram,
            r.peers,
            r.unchoked,
            r.down_bytes_sec >> 10,
            r.variant,
            r.rescues,
            mode,
            r.piece_bytes >> 10
        )
    }
}

/// Times bytes a response hands to the player from start.
#[derive(Clone, Debug)]
pub struct StreamMeter {
    start: Instant,
    pub first: Duration,
    pub m1: Duration,
    pub m2: Duration,
    pub n: i64,
}

impl StreamMeter {
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            first: Duration::ZERO,
            m1: Duration::ZERO,
            m2: Duration::ZERO,
            n: 0,
        }
    }

    /// Note `n` bytes observed at `now`.
    pub fn observe(&mut self, now: Instant, n: usize) {
        if n == 0 {
            return;
        }
        let at = now.saturating_duration_since(self.start);
        let before = self.n;
        self.n += n as i64;
        if before == 0 {
            self.first = at;
        }
        if before < 1 << 20 && self.n >= 1 << 20 {
            self.m1 = at;
        }
        if before < 2 << 20 && self.n >= 2 << 20 {
            self.m2 = at;
        }
    }
}

/// Pick an A/B seek variant name when the experiment is on.
pub fn pick_seek_variant(
    experiment_on: bool,
    start: i64,
    focus_blocks_default: i32,
    coin_focus_128: bool,
    coin_unverified: bool,
) -> (String, bool, i32) {
    if !experiment_on || start < 0 {
        return ("default".into(), false, focus_blocks_default);
    }
    let mut focus = focus_blocks_default;
    if coin_focus_128 {
        focus = 128;
    }
    let unverified = coin_unverified;
    let mode = if unverified { "u" } else { "v" };
    (format!("{mode}-f{focus}"), unverified, focus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_marks_first_1mib_2mib() {
        let start = Instant::now();
        let mut m = StreamMeter::new(start);
        m.observe(start + Duration::from_millis(5), 100);
        assert_eq!(m.first, Duration::from_millis(5));
        m.observe(start + Duration::from_millis(50), 1 << 20);
        assert!(!m.m1.is_zero());
        m.observe(start + Duration::from_millis(80), 1 << 20);
        assert!(!m.m2.is_zero());
    }

    #[test]
    fn format_line_includes_header_columns() {
        let log = SeekLog::new("test");
        let line = log.format_line(&SeekRecord {
            at: "2020-01-01T00:00:00Z".into(),
            torrent: "abc".into(),
            file: 0,
            kind: "range".into(),
            offset: 1 << 20,
            length: 100 << 20,
            first: Duration::from_millis(10),
            m1: Duration::ZERO,
            m2: Duration::ZERO,
            total: Duration::from_millis(100),
            bytes: 100,
            in_ram: true,
            peers: 5,
            unchoked: 2,
            down_bytes_sec: 20480,
            variant: "default".into(),
            rescues: 0,
            direct_play: false,
            piece_bytes: 1 << 20,
        });
        assert!(line.contains("abc"));
        assert!(line.contains("download"));
        assert_eq!(
            SeekLog::header().matches(',').count(),
            line.matches(',').count()
        );
    }
}

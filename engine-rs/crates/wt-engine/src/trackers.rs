//! Public tracker list scraper, validation, caching, and tier management.
//!
//! Port of `engine-go/trackers.go`.

use std::collections::HashSet;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use url::Url;

pub const TRACKER_LIST_SOURCE: &str =
    "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt";
pub const TRACKER_LIST_LIMIT: usize = 20;
pub const TRACKER_SUPPLEMENT_LIMIT: usize = 24;
pub const TRACKER_RESPONSE_LIMIT: usize = 64 * 1024;
pub const TRACKER_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 3600);

pub const DEFAULT_ANNOUNCE: &[&str] = &[
    "wss://tracker.openwebtorrent.com",
    "wss://tracker.webtorrent.dev",
    "wss://tracker.btorrent.xyz",
    "wss://tracker.files.fm:7073/announce",
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://explodie.org:6969/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.moeking.me:6969/announce",
    "udp://opentracker.i2p.rocks:6969/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://tracker.bittor.pw:1337/announce",
    "udp://tracker.internetwarriors.net:1337/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://tracker.altrosky.nl:6969/announce",
    "udp://tracker.tiny-vps.com:6969/announce",
    "udp://tracker.qu.ax:6969/announce",
    "udp://tracker.leechers-paradise.org:6969/announce",
    "http://tracker.openbittorrent.com:80/announce",
    "https://tracker.tamersunion.org:443/announce",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackerCache {
    pub version: i32,
    pub trackers: Vec<String>,
    #[serde(default = "default_time", with = "cache_time")]
    pub checked_at: SystemTime,
    #[serde(default = "default_time", with = "cache_time")]
    pub next_attempt: SystemTime,
    pub failures: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

impl Default for TrackerCache {
    fn default() -> Self {
        Self {
            version: 1,
            trackers: Vec::new(),
            checked_at: SystemTime::UNIX_EPOCH,
            next_attempt: SystemTime::UNIX_EPOCH,
            failures: 0,
            etag: None,
            last_modified: None,
        }
    }
}

fn default_time() -> SystemTime {
    SystemTime::UNIX_EPOCH
}

// Go's cache uses RFC3339 timestamps. Accept the early Rust representation too.
mod cache_time {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
        let (seconds, nanos) = match time.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(e) => {
                let d = e.duration();
                if d.subsec_nanos() == 0 {
                    (-(d.as_secs() as i64), 0)
                } else {
                    (-(d.as_secs() as i64) - 1, 1_000_000_000 - d.subsec_nanos())
                }
            }
        };
        let seconds = seconds as libc::time_t;
        // gmtime_r writes only the initialized tm and does not use process timezone.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::gmtime_r(&seconds, &mut tm) }.is_null() {
            return Err(serde::ser::Error::custom("invalid tracker cache time"));
        }
        let fraction = if nanos == 0 {
            String::new()
        } else {
            format!(".{nanos:09}").trim_end_matches('0').to_string()
        };
        serializer.serialize_str(&format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{fraction}Z",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        ))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text).ok_or_else(|| serde::de::Error::custom("invalid tracker timestamp"))
    }

    fn parse(text: &str) -> Option<SystemTime> {
        let number = |a, b| text.get(a..b)?.parse::<i32>().ok();
        if text.get(4..5)? != "-"
            || text.get(7..8)? != "-"
            || text.get(10..11)? != "T"
            || text.get(13..14)? != ":"
            || text.get(16..17)? != ":"
        {
            return None;
        }
        let (year, month, day, hour, minute, second) = (
            number(0, 4)?,
            number(5, 7)?,
            number(8, 10)?,
            number(11, 13)?,
            number(14, 16)?,
            number(17, 19)?,
        );
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        let mut rest = text.get(19..)?;
        let mut nanos = 0;
        if let Some(fraction) = rest.strip_prefix('.') {
            let count = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if count == 0 || count > 9 {
                return None;
            }
            nanos = fraction.get(..count)?.parse::<u32>().ok()? * 10u32.pow((9 - count) as u32);
            rest = fraction.get(count..)?;
        }
        let offset = if rest == "Z" {
            0
        } else {
            if rest.len() != 6 || rest.get(3..4)? != ":" {
                return None;
            }
            let sign = match rest.get(..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let h = rest.get(1..3)?.parse::<i64>().ok()?;
            let m = rest.get(4..6)?.parse::<i64>().ok()?;
            if h > 23 || m > 59 {
                return None;
            }
            sign * (h * 3600 + m * 60)
        };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = year - 1900;
        tm.tm_mon = month - 1;
        tm.tm_mday = day;
        tm.tm_hour = hour;
        tm.tm_min = minute;
        tm.tm_sec = second;
        let seconds = unsafe { libc::timegm(&mut tm) } as i64;
        if tm.tm_year != year - 1900 || tm.tm_mon != month - 1 || tm.tm_mday != day {
            return None;
        }
        let seconds = seconds.checked_sub(offset)?;
        let base = if seconds >= 0 {
            SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))?
        } else {
            SystemTime::UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))?
        };
        base.checked_add(Duration::from_nanos(nanos as u64))
    }
}

pub struct TrackerPool {
    inner: Arc<Mutex<TrackerPoolInner>>,
    path: PathBuf,
    refresh_lock: tokio::sync::Mutex<()>,
    cancelled: tokio::sync::watch::Sender<bool>,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct TrackerPoolInner {
    cache: TrackerCache,
}

impl TrackerPool {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        let path = dir.as_ref().join("public-trackers-v1.json");
        let mut cache = TrackerCache {
            version: 1,
            ..Default::default()
        };

        use std::io::Read;
        let data = fs::File::open(&path).and_then(|file| {
            let mut data = Vec::new();
            file.take((TRACKER_RESPONSE_LIMIT + 1) as u64)
                .read_to_end(&mut data)?;
            Ok(data)
        });
        if let Ok(data) = data {
            if data.len() <= TRACKER_RESPONSE_LIMIT {
                if let Ok(mut loaded) = serde_json::from_slice::<TrackerCache>(&data) {
                    if loaded.version == 1 {
                        loaded.trackers = parse_tracker_list(&loaded.trackers.join("\n"));
                        let now = SystemTime::now();
                        if loaded.next_attempt > now + TRACKER_REFRESH_INTERVAL
                            || loaded.checked_at > now
                        {
                            loaded.next_attempt = SystemTime::UNIX_EPOCH;
                        }
                        loaded.failures = loaded.failures.clamp(0, 7);
                        if loaded.trackers.is_empty() {
                            loaded.etag = None;
                            loaded.last_modified = None;
                        }
                        cache = loaded;
                    }
                }
            }
        }

        Self {
            inner: Arc::new(Mutex::new(TrackerPoolInner { cache })),
            path,
            refresh_lock: tokio::sync::Mutex::new(()),
            cancelled: tokio::sync::watch::channel(false).0,
            worker: Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        let guard = self.inner.lock();
        if guard.cache.trackers.is_empty() {
            return DEFAULT_ANNOUNCE.iter().map(|s| s.to_string()).collect();
        }

        let mut result: Vec<String> = DEFAULT_ANNOUNCE
            .iter()
            .take(4)
            .map(|s| s.to_string())
            .collect();
        let mut seen: HashSet<String> = result.iter().cloned().collect();

        for tr in &guard.cache.trackers {
            if !seen.contains(tr) && result.len() < TRACKER_SUPPLEMENT_LIMIT {
                result.push(tr.clone());
                seen.insert(tr.clone());
            }
        }
        result
    }

    pub fn cache_snapshot(&self) -> TrackerCache {
        self.inner.lock().cache.clone()
    }

    pub fn save(&self, cache: &TrackerCache) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        fs::create_dir_all(self.path.parent().unwrap())?;
        let tmp_path = self
            .path
            .with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp_path)?;
            file.write_all(&serde_json::to_vec(cache)?)?;
            file.sync_all()?;
            fs::rename(&tmp_path, &self.path)
        })();
        let _ = fs::remove_file(tmp_path);
        result
    }

    pub fn set_trackers(&self, trackers: Vec<String>) {
        let mut guard = self.inner.lock();
        guard.cache.trackers = parse_tracker_list(&trackers.join("\n"));
        let _ = self.save(&guard.cache);
    }

    /// Conditional list refresh; no torrent hash or supplied tracker enters this request/cache.
    pub async fn refresh_from(
        &self,
        client: &reqwest::Client,
        source: &str,
        now: SystemTime,
    ) -> std::io::Result<bool> {
        let _serial = self.refresh_lock.lock().await;
        let mut cancelled = self.cancelled.subscribe();
        if *cancelled.borrow() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let mut cache = self.cache_snapshot();
        if now < cache.next_attempt {
            return Ok(false);
        }
        let fetch = async {
            let mut request = client.get(source).header(
                reqwest::header::USER_AGENT,
                format!("TorrentPlayer/{}", crate::app_version()),
            );
            if let Some(etag) = &cache.etag {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }
            if let Some(modified) = &cache.last_modified {
                request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
            }
            let mut response = request.send().await.map_err(std::io::Error::other)?;
            match response.status().as_u16() {
                304 => {
                    if cache.trackers.is_empty() {
                        return Err(std::io::Error::other("304 without cached trackers"));
                    }
                    Ok(false)
                }
                200 => {
                    let mut body = Vec::new();
                    while let Some(chunk) = response.chunk().await.map_err(std::io::Error::other)? {
                        if body.len() + chunk.len() > TRACKER_RESPONSE_LIMIT {
                            return Err(std::io::Error::other("tracker list exceeds size limit"));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    let trackers = parse_tracker_list(&String::from_utf8_lossy(&body));
                    if trackers.is_empty() {
                        return Err(std::io::Error::other(
                            "tracker list contains no valid public trackers",
                        ));
                    }
                    let changed = cache.trackers != trackers;
                    cache.trackers = trackers;
                    cache.etag = response
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    cache.last_modified = response
                        .headers()
                        .get(reqwest::header::LAST_MODIFIED)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    Ok(changed)
                }
                status => Err(std::io::Error::other(format!(
                    "tracker list HTTP status {status}"
                ))),
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancelled.changed() => return Err(std::io::ErrorKind::Interrupted.into()),
            result = tokio::time::timeout(Duration::from_secs(10), fetch) => result.unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into())),
        };
        if *cancelled.borrow() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        cache.version = 1;
        if result.is_err() {
            cache.failures = (cache.failures + 1).min(7);
            cache.next_attempt = now
                + Duration::from_secs((15 * 60 * (1u64 << (cache.failures - 1))).min(24 * 3600));
        } else {
            cache.failures = 0;
            cache.checked_at = now;
            cache.next_attempt = now + TRACKER_REFRESH_INTERVAL;
        }
        self.inner.lock().cache = cache.clone();
        let path = self.path.clone();
        // Persist off the async executor. Unique temporary paths protect concurrent setters.
        let _ = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let temporary = path.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
            let save = (|| -> std::io::Result<()> {
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .mode(0o600)
                    .write(true)
                    .open(&temporary)?;
                file.write_all(&serde_json::to_vec(&cache)?)?;
                file.sync_all()?;
                fs::rename(&temporary, path)
            })();
            let _ = fs::remove_file(&temporary);
            if let Err(error) = save {
                tracing::warn!(%error, "public tracker cache could not be saved");
            }
        })
        .await;
        result
    }

    pub fn start(
        self: &Arc<Self>,
        handle: &tokio::runtime::Handle,
        source: String,
        on_refresh: Arc<dyn Fn() + Send + Sync>,
    ) {
        let mut worker = self.worker.lock();
        if worker.is_some() || *self.cancelled.borrow() {
            return;
        }
        let pool = self.clone();
        *worker = Some(handle.spawn(async move {
            let client = match reqwest::Client::builder().timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 3 || attempt.url().scheme() != "https" || attempt.url().host_str() != Some("raw.githubusercontent.com") {
                        attempt.error("unexpected tracker-list redirect")
                    } else { attempt.follow() }
                })).build() {
                    Ok(client) => client,
                    Err(error) => { tracing::warn!(%error, "tracker list client"); return; }
                };
            let mut cancelled = pool.cancelled.subscribe();
            loop {
                if *cancelled.borrow() { break; }
                match pool.refresh_from(&client, &source, SystemTime::now()).await {
                    Ok(true) if !*cancelled.borrow() => on_refresh(),
                    Err(error) if !*cancelled.borrow() => tracing::warn!(%error, "public tracker list; retaining cached/fallback trackers"),
                    _ => {},
                }
                let delay = pool.cache_snapshot().next_attempt.duration_since(SystemTime::now()).unwrap_or_default().max(Duration::from_secs(60));
                tokio::select! { _ = cancelled.changed() => break, _ = tokio::time::sleep(delay) => {} }
            }
        }));
    }

    pub fn close(&self) {
        self.cancelled.send_replace(true);
        if let Some(worker) = self.worker.lock().take() {
            worker.abort();
        }
    }
}

impl crate::server::EngineServer {
    pub fn supplement_trackers(
        &self,
        record: &Arc<parking_lot::RwLock<crate::server::TorrentRecord>>,
    ) {
        let mut rec = record.write();
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) || rec.torrent.closed() {
            return;
        }
        let Some(info) = rec.torrent.info() else {
            return;
        };
        if info.private == Some(true) {
            return;
        }
        let mut seen = HashSet::new();
        for tier in rec.torrent.inner.trackers.read().iter() {
            for tracker in tier {
                seen.insert(tracker.clone());
                if let Some(normalized) = public_tracker_url(tracker) {
                    seen.insert(normalized);
                }
            }
        }
        let mut added = Vec::new();
        for tracker in self.trackers.snapshot() {
            if rec.supplemental_trackers >= TRACKER_SUPPLEMENT_LIMIT {
                break;
            }
            if seen.insert(tracker.clone()) {
                added.push(vec![tracker]);
                rec.supplemental_trackers += 1;
            }
        }
        // Rust AddTrackers appends tiers (Go merges by tier index).
        if !added.is_empty() {
            let _runtime = self.client.inner.tokio_handle.enter();
            rec.torrent.add_trackers(added);
        }
    }

    pub fn refresh_torrent_trackers(&self) {
        let records: Vec<_> = self.records.read().values().cloned().collect();
        for rec in records {
            self.supplement_trackers(&rec);
        }
    }
}

pub fn public_tracker_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2048 {
        return None;
    }

    let authority = raw.split_once("://")?.1.split(['/', '?', '#']).next()?;
    if !authority.is_ascii() || authority.contains(['@', '\\']) || raw.contains(['\t', '\r', '\n'])
    {
        return None;
    }
    let mut u = Url::parse(raw).ok()?;
    if !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return None;
    }

    let scheme = u.scheme().to_ascii_lowercase();
    match scheme.as_str() {
        "udp" | "http" | "https" | "ws" | "wss" => {}
        _ => return None,
    }

    let raw_host = u.host_str()?;
    let host = raw_host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host
            .chars()
            .any(|c| matches!(c, '%' | ' ' | '\\' | '\t' | '\r' | '\n'))
    {
        return None;
    }

    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                if v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_multicast()
                    || octets[0] == 0
                {
                    return None;
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback()
                    || v6.is_multicast()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xfe00 == 0xfc00)
                    || (v6.segments()[0] & 0xffc0 == 0xfe80)
                    || v6.to_ipv4_mapped().is_some_and(|v| {
                        v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified()
                    })
                {
                    return None;
                }
            }
        }
    } else {
        if !host.contains('.') {
            return None;
        }
        for suffix in &[
            ".localhost",
            ".local",
            ".internal",
            ".invalid",
            ".test",
            ".onion",
            ".i2p",
        ] {
            if host.ends_with(suffix) {
                return None;
            }
        }
        for label in host.split('.') {
            if label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
            {
                return None;
            }
            for ch in label.chars() {
                if !ch.is_ascii_alphanumeric() && ch != '-' {
                    return None;
                }
            }
        }
    }

    let port = u.port();
    if port == Some(0) {
        return None;
    }
    if scheme == "udp" && port.is_none() {
        return None;
    }

    let normalized_port = match (scheme.as_str(), port) {
        ("https" | "wss", Some(443)) => None,
        ("http" | "ws", Some(80)) => None,
        (_, p) => p,
    };

    let _ = u.set_scheme(&scheme);
    let _ = u.set_host(Some(&host));
    let _ = u.set_port(normalized_port);

    let mut res = u.to_string();
    let remainder = raw.split_once("://")?.1;
    if !remainder.split(['?', '#']).next()?.contains('/') && u.path() == "/" {
        let authority_end =
            res.find("://")? + 3 + res.split_once("://")?.1.find('/').unwrap_or(res.len());
        if res.as_bytes().get(authority_end) == Some(&b'/') {
            res.remove(authority_end);
        }
    }

    Some(res)
}

pub fn parse_tracker_list(body: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for line in body.lines() {
        if let Some(tr) = public_tracker_url(line) {
            if seen.insert(tr.clone()) {
                result.push(tr);
                if result.len() == TRACKER_LIST_LIMIT {
                    break;
                }
            }
        }
    }
    result
}

pub fn clean_supplied_trackers(tiers: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for tier in tiers {
        let mut kept = Vec::new();
        for raw in tier {
            let raw_trimmed = raw.trim();
            if let Ok(mut u) = Url::parse(raw_trimmed) {
                if u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
                    continue;
                }
                let scheme = u.scheme().to_ascii_lowercase();
                match scheme.as_str() {
                    "http" | "https" | "ws" | "wss" | "udp" | "udp4" | "udp6" => {}
                    _ => continue,
                }
                if u.port() == Some(0) || (scheme.starts_with("udp") && u.port().is_none()) {
                    continue;
                }
                u.set_fragment(None);
                let Some((_, remainder)) = raw_trimmed.split_once(':') else {
                    continue;
                };
                let authority = remainder
                    .strip_prefix("//")
                    .and_then(|r| r.split(['/', '?', '#']).next())
                    .unwrap_or("");
                if authority.contains('@') || raw_trimmed.contains(['\t', '\r', '\n', '\\']) {
                    continue;
                }
                let tr = format!(
                    "{}:{}",
                    scheme,
                    remainder.split('#').next().unwrap_or(remainder)
                );
                let host_str = u.host_str().unwrap_or("").to_ascii_lowercase();
                let key = format!(
                    "{}://{}{}{}{}",
                    scheme,
                    host_str,
                    u.port().map(|p| format!(":{p}")).unwrap_or_default(),
                    u.path(),
                    u.query().map(|q| format!("?{q}")).unwrap_or_default()
                );
                if seen.insert(key) {
                    kept.push(tr);
                }
            }
        }
        if !kept.is_empty() {
            result.push(kept);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_tracker_url() {
        assert_eq!(
            public_tracker_url("udp://tracker.opentrackr.org:1337/announce"),
            Some("udp://tracker.opentrackr.org:1337/announce".to_string())
        );
        // Invalid UDP without port
        assert_eq!(public_tracker_url("udp://tracker.example.com"), None);
        // Invalid scheme
        assert_eq!(public_tracker_url("ftp://tracker.example.com"), None);
    }

    #[test]
    fn test_clean_supplied_trackers() {
        let input = vec![
            vec![
                "udp://tracker.opentrackr.org:1337/announce".to_string(),
                "udp://tracker.opentrackr.org:1337/announce".to_string(), // duplicate
            ],
            vec![
                "not a url".to_string(),
                "https://tracker.tamersunion.org:443/announce".to_string(),
            ],
        ];
        let cleaned = clean_supplied_trackers(&input);
        assert_eq!(cleaned.len(), 2);
        assert_eq!(cleaned[0].len(), 1);
        assert_eq!(cleaned[1].len(), 1);
    }

    #[test]
    fn test_tracker_pool_snapshot() {
        let pool = TrackerPool::new("/home/kar/.cache/wt-tmp/codex-server/offline-pool");
        let snapshot = pool.snapshot();
        assert!(!snapshot.is_empty());
        assert!(snapshot.contains(&"udp://tracker.opentrackr.org:1337/announce".to_string()));
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn cache_accepts_go_rfc3339_and_preserves_private_tracker_identity() {
        let raw = r#"{"version":1,"trackers":["https://tracker.example.org"],"checkedAt":"2026-10-27T18:04:56.123456789+05:30","nextAttempt":"2026-10-28T12:34:56Z","failures":0}"#;
        let cache: TrackerCache = serde_json::from_str(raw).unwrap();
        let encoded = serde_json::to_value(&cache).unwrap();
        assert_eq!(encoded["checkedAt"], "2026-10-27T12:34:56.123456789Z");
        assert_eq!(encoded["nextAttempt"], "2026-10-28T12:34:56Z");
        let supplied = vec![vec![
            "https://Private.Example.org:443?passkey=One".into(),
            "https://Private.Example.org:443?passkey=Two".into(),
        ]];
        assert_eq!(clean_supplied_trackers(&supplied), supplied);
        assert_eq!(
            public_tracker_url("https://tracker.example.org?x=1"),
            Some("https://tracker.example.org?x=1".into())
        );
        assert!(public_tracker_url("https://@tracker.example.org").is_none());
    }

    #[tokio::test]
    async fn conditional_refresh_backoff_bounds_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let server = tokio::spawn(async move {
            for mode in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = vec![0; 8192];
                let n = stream.read(&mut input).await.unwrap();
                let request = String::from_utf8_lossy(&input[..n]).to_ascii_lowercase();
                assert!(
                    request.starts_with("get / http/1.1"),
                    "torrent parameters leaked: {request}"
                );
                observed.fetch_add(1, Ordering::SeqCst);
                let (status, body) = match mode {
                    0 => ("200 OK", "udp://tracker.example.org:1337/announce\n".into()),
                    1 => {
                        assert!(request.contains("if-none-match: \"list-v1\""));
                        assert!(
                            request.contains("if-modified-since: mon, 14 sep 2026 00:00:00 gmt")
                        );
                        ("304 Not Modified", String::new())
                    }
                    2 => ("503 Unavailable", String::new()),
                    3 => ("200 OK", "<html>temporary error</html>".into()),
                    _ => ("200 OK", "x".repeat(TRACKER_RESPONSE_LIMIT + 1)),
                };
                let reply = format!("HTTP/1.1 {status}\r\nETag: \"list-v1\"\r\nLast-Modified: Mon, 14 Sep 2026 00:00:00 GMT\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let client = reqwest::Client::new();
        let pool = TrackerPool::new(dir.path());
        let mut now = SystemTime::now();
        assert!(pool.refresh_from(&client, &source, now).await.unwrap());
        let expected = pool.snapshot();
        assert_eq!(expected.len(), 5);
        assert_eq!(TrackerPool::new(dir.path()).snapshot(), expected);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&pool.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!pool
            .refresh_from(&client, &source, now + Duration::from_secs(3600))
            .await
            .unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        now += TRACKER_REFRESH_INTERVAL;
        assert!(!pool.refresh_from(&client, &source, now).await.unwrap());
        for failures in 1..=3 {
            now += TRACKER_REFRESH_INTERVAL;
            assert!(pool.refresh_from(&client, &source, now).await.is_err());
            assert_eq!(pool.snapshot(), expected);
            assert_eq!(pool.cache_snapshot().failures, failures);
            let before = calls.load(Ordering::SeqCst);
            assert!(!pool
                .refresh_from(&client, &source, now + Duration::from_secs(60))
                .await
                .unwrap());
            assert_eq!(calls.load(Ordering::SeqCst), before);
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_interrupts_in_flight_fetch_without_overwriting_cache() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = format!("http://{}", listener.local_addr().unwrap());
        let pool = Arc::new(TrackerPool::new(dir.path()));
        let fetch_pool = pool.clone();
        let task = tokio::spawn(async move {
            fetch_pool
                .refresh_from(&reqwest::Client::new(), &source, SystemTime::now())
                .await
        });
        let (_stream, _) = listener.accept().await.unwrap();
        pool.close();
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(pool.cache_snapshot().failures, 0);
        assert!(!pool.path.exists());
    }
}

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
    #[serde(default = "default_time")]
    pub checked_at: SystemTime,
    #[serde(default = "default_time")]
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

pub struct TrackerPool {
    inner: Arc<Mutex<TrackerPoolInner>>,
    path: PathBuf,
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

        if let Ok(data) = fs::read(&path) {
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

    pub fn save(&self, cache: &TrackerCache) -> std::io::Result<()> {
        let data = serde_json::to_vec(cache)?;
        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, data)?;
        fs::rename(tmp_path, &self.path)?;
        Ok(())
    }

    pub fn set_trackers(&self, trackers: Vec<String>) {
        let mut guard = self.inner.lock();
        guard.cache.trackers = trackers;
        let _ = self.save(&guard.cache);
    }
}

pub fn public_tracker_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2048 {
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

    if let Ok(ip) = host.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                if v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || octets[0] == 0
                {
                    return None;
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback() || v6.is_multicast() {
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

    Some(u.to_string())
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
                if scheme.starts_with("udp") && u.port().is_none() {
                    continue;
                }
                u.set_fragment(None);
                let tr = u.to_string();
                let host_str = u.host_str().unwrap_or("").to_ascii_lowercase();
                let key = format!("{}://{}{}", scheme, host_str, u.path());
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
        let pool = TrackerPool::new("/tmp/wt-trackers-test");
        let snapshot = pool.snapshot();
        assert!(!snapshot.is_empty());
        assert!(snapshot.contains(&"udp://tracker.opentrackr.org:1337/announce".to_string()));
    }
}

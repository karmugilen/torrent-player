//! Download torrents.csv and fetch torrents-csv catalog pages.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const DUMP_URL: &str =
    "https://codeberg.org/heretic/torrents-csv-data/raw/branch/main/torrents.csv";
pub const CATALOG_ENDPOINT: &str = "https://torrents-csv.com/service/search";
pub const DUMP_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// Ensure `path` holds a fresh dump. Returns 0 on success, -1 on failure.
pub fn ensure_dump(path: &Path, force: bool) -> Result<(), String> {
    if !force {
        if let Ok(meta) = fs::metadata(path) {
            if meta.len() > 1024 {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = SystemTime::now().duration_since(modified) {
                        if age.as_secs() < DUMP_MAX_AGE_SECS {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("csv.tmp");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30 * 60))
        .user_agent("TorrentAtlas/1.0 (metadata-only; rust dump)")
        .build();
    let resp = agent
        .get(DUMP_URL)
        .call()
        .map_err(|e| format!("dump download: {e}"))?;
    if resp.status() != 200 {
        return Err(format!("dump HTTP {}", resp.status()));
    }
    let mut reader = resp.into_reader();
    let mut out = File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CatalogTorrent {
    pub infohash: String,
    pub name: String,
    pub size_bytes: i64,
    pub seeders: i32,
    pub leechers: i32,
}

#[derive(Debug, Clone)]
pub struct CatalogPage {
    pub torrents: Vec<CatalogTorrent>,
    pub next: Option<i32>,
}

/// Fetch one catalog search page. Uses HTTP/1.1-friendly ureq defaults.
pub fn fetch_catalog_page(query: &str, size: i32, after: i32) -> Result<CatalogPage, String> {
    let size = size.clamp(1, 50);
    let mut url = format!(
        "{CATALOG_ENDPOINT}?q={}&size={size}",
        urlencoding_encode(query)
    );
    if after > 0 {
        url.push_str(&format!("&after={after}"));
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout_read(Duration::from_secs(8))
        .user_agent("TorrentAtlas/1.0 (metadata-only; rust catalog)")
        .build();
    let resp = agent
        .get(&url)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| format!("catalog: {e}"))?;
    if resp.status() != 200 {
        return Err(format!("catalog HTTP {}", resp.status()));
    }
    let body = resp.into_string().map_err(|e| e.to_string())?;
    parse_catalog_json(&body)
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Minimal JSON extract for the known catalog shape (no serde dependency).
fn parse_catalog_json(body: &str) -> Result<CatalogPage, String> {
    let bytes = body.as_bytes();
    let mut torrents = Vec::new();
    let mut next: Option<i32> = None;

    // Find "next": <num|null>
    if let Some(i) = find_key(bytes, b"\"next\"") {
        let rest = &bytes[i..];
        if let Some(v) = parse_json_number_after_colon(rest) {
            next = Some(v as i32);
        }
    }

    let mut search_from = 0usize;
    while let Some(rel) = find_key(&bytes[search_from..], b"\"infohash\"") {
        let start = search_from + rel;
        let obj_start = bytes[..start].iter().rposition(|&c| c == b'{').unwrap_or(start);
        let obj_end = match bytes[start..].iter().position(|&c| c == b'}') {
            Some(n) => start + n + 1,
            None => break,
        };
        let obj = &bytes[obj_start..obj_end];
        if let Some(t) = parse_torrent_object(obj) {
            torrents.push(t);
        }
        search_from = obj_end;
    }
    Ok(CatalogPage { torrents, next })
}

fn find_key(hay: &[u8], key: &[u8]) -> Option<usize> {
    hay.windows(key.len()).position(|w| w == key)
}

fn parse_torrent_object(obj: &[u8]) -> Option<CatalogTorrent> {
    let infohash = json_string_value(obj, b"\"infohash\"")?;
    let name = json_string_value(obj, b"\"name\"")?;
    if infohash.len() != 40 {
        return None;
    }
    let size_bytes = json_i64_value(obj, b"\"size_bytes\"").unwrap_or(0);
    let seeders = json_i64_value(obj, b"\"seeders\"").unwrap_or(0) as i32;
    let leechers = json_i64_value(obj, b"\"leechers\"").unwrap_or(0) as i32;
    Some(CatalogTorrent {
        infohash: infohash.to_ascii_lowercase(),
        name,
        size_bytes,
        seeders,
        leechers,
    })
}

fn json_string_value(obj: &[u8], key: &[u8]) -> Option<String> {
    let i = find_key(obj, key)?;
    let after = &obj[i + key.len()..];
    let colon = after.iter().position(|&c| c == b':')?;
    let mut p = colon + 1;
    while p < after.len() && after[p].is_ascii_whitespace() {
        p += 1;
    }
    if p >= after.len() || after[p] != b'"' {
        return None;
    }
    p += 1;
    let start = p;
    let mut out = Vec::new();
    while p < after.len() {
        match after[p] {
            b'\\' if p + 1 < after.len() => {
                match after[p + 1] {
                    b'"' | b'\\' | b'/' => {
                        out.push(after[p + 1]);
                        p += 2;
                    }
                    b'n' => {
                        out.push(b'\n');
                        p += 2;
                    }
                    b'r' => {
                        out.push(b'\r');
                        p += 2;
                    }
                    b't' => {
                        out.push(b'\t');
                        p += 2;
                    }
                    b'u' if p + 5 < after.len() => {
                        // Skip unicode escape roughly.
                        p += 6;
                    }
                    _ => {
                        out.push(after[p + 1]);
                        p += 2;
                    }
                }
            }
            b'"' => break,
            c => {
                out.push(c);
                p += 1;
            }
        }
        if out.len() > 512 {
            break;
        }
    }
    let _ = start;
    String::from_utf8(out).ok()
}

fn json_i64_value(obj: &[u8], key: &[u8]) -> Option<i64> {
    let i = find_key(obj, key)?;
    parse_json_number_after_colon(&obj[i + key.len()..])
}

fn parse_json_number_after_colon(after_key: &[u8]) -> Option<i64> {
    let colon = after_key.iter().position(|&c| c == b':')?;
    let mut p = colon + 1;
    while p < after_key.len() && after_key[p].is_ascii_whitespace() {
        p += 1;
    }
    if after_key[p..].starts_with(b"null") {
        return None;
    }
    let start = p;
    if p < after_key.len() && (after_key[p] == b'-' || after_key[p].is_ascii_digit()) {
        p += 1;
        while p < after_key.len() && after_key[p].is_ascii_digit() {
            p += 1;
        }
        return std::str::from_utf8(&after_key[start..p])
            .ok()
            .and_then(|s| s.parse().ok());
    }
    None
}

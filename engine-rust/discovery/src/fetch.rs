//! Download torrents.csv and fetch torrents-csv catalog pages.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
static CATALOG_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

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
    // Reuse TLS sessions and idle sockets across paced worker requests.
    let agent = CATALOG_AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(4))
            .timeout_read(Duration::from_secs(8))
            .user_agent("TorrentAtlas/1.0 (metadata-only; rust catalog)")
            .build()
    });
    let resp = agent
        .get(&url)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| format!("catalog: {e}"))?;
    if resp.status() != 200 {
        return Err(format!("catalog HTTP {}", resp.status()));
    }
    const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;
    let mut body = String::new();
    resp.into_reader()
        .take(MAX_CATALOG_BYTES + 1)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() as u64 > MAX_CATALOG_BYTES {
        return Err("oversized catalog response".to_string());
    }
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

/// Parse actual JSON rather than searching delimiters inside torrent names.
/// Reject challenge pages and malformed responses so Go can try its fallback.
fn parse_catalog_json(body: &str) -> Result<CatalogPage, String> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let rows = value
        .get("torrents")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "catalog response has no torrents array".to_string())?;
    let next = match value.get("next") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .filter(|v| *v >= 0)
                .ok_or_else(|| "invalid catalog cursor".to_string())?,
        ),
    };
    let mut torrents = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(infohash) = row.get("infohash").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(name) = row.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if infohash.len() != 40
            || !infohash.bytes().all(|b| b.is_ascii_hexdigit())
            || name.trim().is_empty()
        {
            continue;
        }
        torrents.push(CatalogTorrent {
            infohash: infohash.to_ascii_lowercase(),
            name: name.to_owned(),
            size_bytes: row
                .get("size_bytes")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .max(0),
            seeders: row
                .get("seeders")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .clamp(0, i32::MAX as i64) as i32,
            leechers: row
                .get("leechers")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .clamp(0, i32::MAX as i64) as i32,
        });
    }
    Ok(CatalogPage { torrents, next })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_preserves_unicode_escaped_quotes_and_braces() {
        let body = r#"{"torrents":[{"infohash":"0123456789abcdef0123456789abcdef01234567","name":"{Show} \"S01\" \u65e5\u672c\u8a9e \ud83c\udfac.mkv","size_bytes":42,"seeders":3,"leechers":1}],"next":25}"#;
        let page = parse_catalog_json(body).unwrap();
        assert_eq!(page.torrents.len(), 1);
        assert_eq!(page.torrents[0].name, "{Show} \"S01\" 日本語 🎬.mkv");
        assert_eq!(page.next, Some(25));
    }

    #[test]
    fn catalog_rejects_html_and_malformed_json() {
        assert!(parse_catalog_json("<html>challenge</html>").is_err());
        assert!(parse_catalog_json(r#"{"torrents":["#).is_err());
    }
}

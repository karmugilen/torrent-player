//! HTTP client for the engine control + stream APIs (localhost only).

use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn null_as_default<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    let opt = Option::<T>::deserialize(de)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Clone, Debug)]
pub struct EngineClient {
    pub ctl_port: u16,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl EngineClient {
    pub fn new(ctl_port: u16) -> Self {
        Self {
            ctl_port,
            timeout: Duration::from_secs(60),
        }
    }

    pub fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self.request("GET", path, None)?;
        if !(200..300).contains(&resp.status) {
            return Err(format!(
                "GET {path} -> {} {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            ));
        }
        serde_json::from_slice(&resp.body).map_err(|e| e.to_string())
    }

    pub fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let payload = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let resp = self.request("POST", path, Some(&payload))?;
        if !(200..300).contains(&resp.status) {
            return Err(format!(
                "POST {path} -> {} {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            ));
        }
        serde_json::from_slice(&resp.body).map_err(|e| e.to_string())
    }

    pub fn post_ok<B: Serialize>(&self, path: &str, body: &B) -> Result<(), String> {
        let _: serde_json::Value = self.post_json(path, body)?;
        Ok(())
    }

    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, String> {
        let addr = format!("127.0.0.1:{}", self.ctl_port);
        let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(|e| e.to_string())?;

        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if let Some(b) = body {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", b.len()));
        }
        req.push_str("\r\n");
        stream
            .write_all(req.as_bytes())
            .map_err(|e| e.to_string())?;
        if let Some(b) = body {
            stream.write_all(b).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        parse_http_response(&buf)
    }
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "incomplete HTTP response".to_string())?;
    let head = std::str::from_utf8(&raw[..sep]).map_err(|e| e.to_string())?;
    let body = raw[sep + 4..].to_vec();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {head}"))?;
    Ok(HttpResponse { status, body })
}

/// Streaming Range GET against the media stream server.
pub fn stream_range(
    stream_port: u16,
    path: &str,
    start: u64,
    end_inclusive: Option<u64>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    stream_range_timed(stream_port, path, start, end_inclusive, timeout).map(|(body, _)| body)
}

/// Returns the body and time until the first response body byte arrives.
pub fn stream_range_timed(
    stream_port: u16,
    path: &str,
    start: u64,
    end_inclusive: Option<u64>,
    timeout: Duration,
) -> Result<(Vec<u8>, Duration), String> {
    let started = Instant::now();
    let addr = format!("127.0.0.1:{stream_port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let range = match end_inclusive {
        Some(e) => format!("bytes={start}-{e}"),
        None => format!("bytes={start}-"),
    };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: {range}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let first_byte = loop {
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            if let Ok(resp) = parse_http_response(&buf) {
                return Err(format!(
                    "stream ended before response body: HTTP {} {}",
                    resp.status,
                    String::from_utf8_lossy(&resp.body)
                ));
            }
            return Err(format!(
                "stream ended before response body (read {} bytes: {:?})",
                buf.len(),
                String::from_utf8_lossy(&buf)
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if buf.len() > sep + 4 {
                break started.elapsed();
            }
        }
    };
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let resp = parse_http_response(&buf)?;
    if resp.status != 200 && resp.status != 206 {
        return Err(format!(
            "stream GET {path} -> {} {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
        ));
    }
    Ok((resp.body, first_byte))
}

// ---- wire types (subset of engine-go/types.go) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsResponse {
    pub download_speed: i64,
    pub upload_speed: i64,
    pub progress: f64,
    pub torrents: i64,
    pub ctl_port: i64,
    pub stream_port: i64,
    pub path: String,
    pub listen_port: i64,
    #[serde(default)]
    pub payload_transfers_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torrent_id: Option<String>,
    pub prepare: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torrent_data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResponse {
    pub id: String,
    pub info_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureRequest {
    pub id: String,
    pub descriptors: Vec<Option<i32>>,
    pub selected: Vec<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_cache_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayRequest {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayResponse {
    pub id: String,
    pub file_index: i32,
    pub name: String,
    pub length: i64,
    pub stream_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub index: i32,
    pub name: String,
    pub path: String,
    pub length: i64,
    pub progress: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentView {
    pub id: String,
    pub info_hash: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub download_speed: i64,
    #[serde(default)]
    pub upload_speed: i64,
    #[serde(default)]
    pub num_peers: i32,
    #[serde(default)]
    pub length: i64,
    #[serde(default)]
    pub downloaded: i64,
    #[serde(default)]
    pub verified_bytes: i64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub files: Vec<FileView>,
    #[serde(default)]
    pub configured: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub selected: Vec<i32>,
    #[serde(default)]
    pub checking: bool,
    #[serde(default)]
    pub checked_pieces: i32,
    #[serde(default)]
    pub check_total: i32,
    #[serde(default)]
    pub seeding: bool,
    pub error: Option<String>,
}

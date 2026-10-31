//! BEP 19 HTTP web seeds: range fetches for piece file spans.
//!
//! Ports `engine-go/patches/torrent/webseed/`.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::Semaphore;
use wt_types::{FileInfo, Info};


/// Discard at most this many leading bytes when a server ignores Range
/// (Go `MaxDiscardBytes` = 48 KiB).
pub const MAX_DISCARD_BYTES: u64 = 48 << 10;

#[derive(Clone, Debug, thiserror::Error)]
pub enum WebSeedError {
    #[error("http: {0}")]
    Http(String),
    #[error("unexpected status {0}")]
    Status(u16),
    #[error("range not satisfiable")]
    Range,
    #[error("too many discard bytes")]
    Discard,
    #[error("closed")]
    Closed,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, WebSeedError>;

/// Byte range within the concatenated torrent payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub start: i64,
    pub length: i64,
}

impl Extent {
    pub fn end(self) -> i64 {
        self.start + self.length
    }
}

/// One file span overlapping an extent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpan {
    pub file_index: usize,
    pub file_offset: i64,
    pub length: i64,
    pub file_length: i64,
}

/// Path escaping for appending file paths to a directory webseed URL.
pub type PathEscaper = Arc<dyn Fn(&[String]) -> String + Send + Sync>;

pub fn default_path_escaper(path_comps: &[String]) -> String {
    path_comps
        .iter()
        .map(|c| {
            let esc = urlencoding_path(c);
            esc.replace('+', "%2B")
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn urlencoding_path(s: &str) -> String {
    // Path-escape each component (similar to Go url.PathEscape + S3 + fix).
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0xf) as usize]));
            }
        }
    }
    out
}

/// Build the URL for a file index per BEP 19.
pub fn url_for_file_index(
    base: &str,
    file_index: usize,
    info: &Info,
    escaper: Option<&PathEscaper>,
) -> String {
    if !base.ends_with('/') {
        return base.to_string();
    }
    let file = &info.files[file_index];
    let mut comps = Vec::with_capacity(1 + file.path.len());
    comps.push(info.name.clone());
    comps.extend(file.path.iter().cloned());
    let trail = match escaper {
        Some(e) => e(&comps),
        None => default_path_escaper(&comps),
    };
    format!("{base}{trail}")
}

/// Locate file spans covering a torrent-byte extent.
pub fn locate_spans(files: &[FileInfo], extent: Extent) -> Vec<FileSpan> {
    let mut out = Vec::new();
    let mut remaining_start = extent.start;
    let mut remaining_len = extent.length;
    for (i, f) in files.iter().enumerate() {
        if remaining_len <= 0 {
            break;
        }
        let file_start = f.torrent_offset;
        let file_end = file_start + f.length;
        if remaining_start >= file_end || remaining_start + remaining_len <= file_start {
            continue;
        }
        let overlap_start = remaining_start.max(file_start);
        let overlap_end = (remaining_start + remaining_len).min(file_end);
        let length = overlap_end - overlap_start;
        if length <= 0 {
            continue;
        }
        out.push(FileSpan {
            file_index: i,
            file_offset: overlap_start - file_start,
            length,
            file_length: f.length,
        });
        remaining_start = overlap_end;
        remaining_len = extent.end() - remaining_start;
    }
    out
}

#[derive(Clone)]
pub struct WebSeedClient {
    url: String,
    http: reqwest::Client,
    info: Option<Info>,
    escaper: Option<PathEscaper>,
    limiter: Arc<Semaphore>,
}

impl WebSeedClient {
    pub fn new(url: impl Into<String>, max_concurrent: usize) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .use_rustls_tls()
            .build()
            .map_err(|e| WebSeedError::Http(e.to_string()))?;
        Ok(Self {
            url: url.into(),
            http,
            info: None,
            escaper: None,
            limiter: Arc::new(Semaphore::new(max_concurrent.max(1))),
        })
    }

    pub fn set_info(&mut self, info: Info) {
        self.info = Some(info);
    }



    /// Fetch `extent` (torrent-relative) and return concatenated bytes.
    pub async fn fetch_extent(&self, extent: Extent) -> Result<bytes::Bytes> {
        let info = self
            .info
            .as_ref()
            .ok_or_else(|| WebSeedError::Other("info not set".into()))?;
        let spans = locate_spans(&info.files, extent);
        if spans.is_empty() {
            return Ok(bytes::Bytes::new());
        }
        let mut out = Vec::with_capacity(extent.length.max(0) as usize);
        for span in spans {
            let chunk = self.fetch_span(&span).await?;
            out.extend_from_slice(&chunk);
        }
        Ok(bytes::Bytes::from(out))
    }

    async fn fetch_span(&self, span: &FileSpan) -> Result<Vec<u8>> {
        let _permit = self
            .limiter
            .acquire()
            .await
            .map_err(|_| WebSeedError::Closed)?;
        let info = self.info.as_ref().unwrap();
        let url = url_for_file_index(&self.url, span.file_index, info, self.escaper.as_ref());
        let mut builder = self.http.get(&url);
        if span.file_offset != 0 || span.length != span.file_length {
            let end = span.file_offset + span.length - 1;
            builder = builder.header(
                reqwest::header::RANGE,
                format!("bytes={}-{}", span.file_offset, end),
            );
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| WebSeedError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        match status {
            200 | 206 => {}
            416 => return Err(WebSeedError::Range),
            s => return Err(WebSeedError::Status(s)),
        }

        // If the server ignored Range and returned 200, optionally discard.
        let mut discard = 0u64;
        if status == 200 && span.file_offset > 0 {
            discard = span.file_offset as u64;
            if discard > MAX_DISCARD_BYTES {
                return Err(WebSeedError::Discard);
            }
        }

        let mut body = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| WebSeedError::Http(e.to_string()))?;
            let mut data = chunk.as_ref();
            if discard > 0 {
                let skip = discard.min(data.len() as u64) as usize;
                data = &data[skip..];
                discard -= skip as u64;
            }
            body.extend_from_slice(data);
            if body.len() as i64 >= span.length {
                body.truncate(span.length as usize);
                break;
            }
        }
        if (body.len() as i64) < span.length {
            return Err(WebSeedError::Other(format!(
                "short body: got {} want {}",
                body.len(),
                span.length
            )));
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_types::FileInfo;

    fn sample_info() -> Info {
        Info {
            name: "dir".into(),
            piece_length: 16384,
            pieces: vec![0u8; 20],
            files: vec![
                FileInfo {
                    path: vec!["a.txt".into()],
                    length: 100,
                    torrent_offset: 0,
                    ..Default::default()
                },
                FileInfo {
                    path: vec!["b.txt".into()],
                    length: 50,
                    torrent_offset: 100,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn locate_across_files() {
        let info = sample_info();
        let spans = locate_spans(
            &info.files,
            Extent {
                start: 90,
                length: 20,
            },
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].file_index, 0);
        assert_eq!(spans[0].file_offset, 90);
        assert_eq!(spans[0].length, 10);
        assert_eq!(spans[1].file_index, 1);
        assert_eq!(spans[1].file_offset, 0);
        assert_eq!(spans[1].length, 10);
    }

    #[test]
    fn url_append_path() {
        let info = sample_info();
        let u = url_for_file_index("http://example/seed/", 0, &info, None);
        assert_eq!(u, "http://example/seed/dir/a.txt");
        let u2 = url_for_file_index("http://example/seed", 0, &info, None);
        assert_eq!(u2, "http://example/seed");
    }

    #[test]
    fn escape_plus() {
        let s = default_path_escaper(&["a+b".into()]);
        assert!(s.contains("%2B"), "{s}");
    }
}

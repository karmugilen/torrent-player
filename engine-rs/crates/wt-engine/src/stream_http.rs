//! HTTP stream server for media playback (`GET`/`HEAD` `/torrent/{id}/file/{index}`).
//!
//! Port of `engine-go/server.go` `handleStream` + Range/`ServeContent` behaviour
//! from `playback.go`. External players (VLC, MX, mpv, Just Player) need
//! `Accept-Ranges`, `Content-Range`, single-range `206`, and prompt cancel on
//! client disconnect.
//!
//! The engine supplies a [`StreamSource`]; this module owns the hyper server,
//! URL parsing, Range math, and MIME headers.

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
// AtomicBool used by BytesStreamReader.

use bytes::Bytes;
use futures::channel::mpsc;
use futures::SinkExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::header::{
    ACCEPT_RANGES, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL,
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use wt_policy::is_video_file_name;

/// Default readahead used when opening a stream (Go playback constants).
pub const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Seekable byte source for one file open.
pub trait StreamReader: Send + Sync {
    fn length(&self) -> i64;
    fn display_name(&self) -> &str;
    fn read_at(&self, buf: &mut [u8], offset: i64) -> io::Result<usize>;
    fn close(&self);
    fn is_closed(&self) -> bool {
        false
    }
}

/// Resolves `/torrent/{id}/file/{index}` opens.
pub trait StreamSource: Send + Sync {
    fn open(&self, id: &str, file_index: usize) -> io::Result<Arc<dyn StreamReader>>;
}

pub struct StreamServer {
    port: u16,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl StreamServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn close(&self) {
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }
    }
}

/// Bind `127.0.0.1:port` (`0` = ephemeral) and serve stream requests.
pub async fn start_stream_server(
    port: u16,
    source: Arc<dyn StreamSource>,
) -> io::Result<Arc<StreamServer>> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
    let bound_port = listener.local_addr()?.port();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server = Arc::new(StreamServer {
        port: bound_port,
        shutdown_tx: Mutex::new(Some(shutdown_tx)),
    });

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                res = listener.accept() => {
                    match res {
                        Ok((stream, _)) => {
                            let io = TokioIo::new(stream);
                            let source = source.clone();
                            tokio::spawn(async move {
                                let service = service_fn(move |req| {
                                    handle_stream_request(req, source.clone())
                                });
                                let _ = Builder::new(TokioExecutor::new())
                                    .serve_connection(io, service)
                                    .await;
                            });
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });

    Ok(server)
}

/// Parse `/torrent/{id}/file/{index}` — the Go URL scheme.
pub fn parse_stream_path(path: &str) -> Option<(&str, usize)> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 4 || parts[0] != "torrent" || parts[2] != "file" {
        return None;
    }
    let idx = parts[3].parse().ok()?;
    Some((parts[1], idx))
}

/// First byte of a single `bytes=N-` / `bytes=N-M` range, or `-1` when the
/// header is absent / multi-range / suffix-only (Go `rangeStart`).
pub fn range_start(header: &str) -> i64 {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return -1;
    };
    if spec.contains(',') {
        return -1;
    }
    let Some((first, _)) = spec.split_once('-') else {
        return -1;
    };
    if first.is_empty() {
        return -1;
    }
    match first.trim().parse::<i64>() {
        Ok(n) if n >= 0 => n,
        _ => -1,
    }
}

/// Full inclusive `(start, end)` for `ServeContent`-style single ranges.
pub fn parse_range(header: &str, file_len: i64) -> Option<(i64, i64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_str, end_str) = spec.split_once('-')?;
    if start_str.is_empty() {
        let suffix_len: i64 = end_str.trim().parse().ok()?;
        if suffix_len <= 0 || file_len <= 0 {
            return None;
        }
        let start = (file_len - suffix_len).max(0);
        let end = file_len.saturating_sub(1);
        Some((start, end))
    } else {
        let start: i64 = start_str.trim().parse().ok()?;
        if start < 0 || (file_len > 0 && start >= file_len) {
            return None;
        }
        if file_len == 0 {
            return None;
        }
        let end = if end_str.trim().is_empty() {
            file_len.saturating_sub(1)
        } else {
            let e: i64 = end_str.trim().parse().ok()?;
            e.min(file_len - 1)
        };
        if start > end {
            return None;
        }
        Some((start, end))
    }
}

/// Content-Type for a file display name. Prefer a concrete video MIME when
/// [`is_video_file_name`] says so; otherwise map common audio/containers.
pub fn content_type_for_name(name: &str) -> &'static str {
    let ext = name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" | "m4v" | "mov" => "video/mp4",
        "mkv" | "mka" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "ts" | "m2ts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "aac" | "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "srt" => "application/x-subrip",
        "vtt" => "text/vtt",
        _ if is_video_file_name(name) => "video/mp4",
        _ => "application/octet-stream",
    }
}

fn empty_body() -> BoxBody<Bytes, io::Error> {
    Full::new(Bytes::new()).map_err(|e| match e {}).boxed()
}

fn text_body(msg: &'static str) -> BoxBody<Bytes, io::Error> {
    Full::new(Bytes::from_static(msg.as_bytes()))
        .map_err(|e| match e {})
        .boxed()
}

async fn handle_stream_request(
    req: Request<hyper::body::Incoming>,
    source: Arc<dyn StreamSource>,
) -> Result<Response<BoxBody<Bytes, io::Error>>, Infallible> {
    let path = req.uri().path();
    let Some((id, file_index)) = parse_stream_path(path) else {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(text_body("Not Found"))
            .unwrap());
    };

    let reader = match source.open(id, file_index) {
        Ok(r) => r,
        Err(e) => {
            let status = match e.kind() {
                io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
                io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
                io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
                _ => StatusCode::CONFLICT,
            };
            return Ok(Response::builder()
                .status(status)
                .body(
                    Full::new(Bytes::from(e.to_string()))
                        .map_err(|e| match e {})
                        .boxed(),
                )
                .unwrap());
        }
    };

    let file_length = reader.length();
    let mime = content_type_for_name(reader.display_name());
    let range_header = req.headers().get(RANGE).and_then(|v| v.to_str().ok());

    let (start, end, status) = if let Some(range_str) = range_header {
        match parse_range(range_str, file_length) {
            Some(r) => (r.0, r.1, StatusCode::PARTIAL_CONTENT),
            None => {
                reader.close();
                return Ok(Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(CONTENT_RANGE, format!("bytes */{file_length}"))
                    .header(ACCEPT_RANGES, "bytes")
                    .body(empty_body())
                    .unwrap());
            }
        }
    } else {
        (0, file_length.saturating_sub(1), StatusCode::OK)
    };

    let content_length = if file_length == 0 { 0 } else { end - start + 1 };

    let (mut tx, rx) = mpsc::channel::<Result<Frame<Bytes>, io::Error>>(4);
    let body_stream = StreamBody::new(rx);

    if req.method() != Method::HEAD && content_length > 0 {
        let reader_clone = reader.clone();
        tokio::spawn(async move {
            let mut off = start;
            while off <= end {
                if reader_clone.is_closed() {
                    break;
                }
                let want = ((end - off + 1) as usize).min(STREAM_CHUNK_BYTES);
                let reader_ref = reader_clone.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let mut buf = vec![0u8; want];
                    match reader_ref.read_at(&mut buf, off) {
                        Ok(n) if n > 0 => {
                            buf.truncate(n);
                            Ok(Some(buf))
                        }
                        Ok(_) => Ok(None),
                        Err(e) => Err(e),
                    }
                })
                .await;
                match result {
                    Ok(Ok(Some(buf))) => {
                        let n = buf.len() as i64;
                        if tx.send(Ok(Frame::data(Bytes::from(buf)))).await.is_err() {
                            break;
                        }
                        off += n;
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(e)) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                    Err(_) => break,
                }
            }
            reader_clone.close();
        });
    } else {
        reader.close();
    }

    let mut builder = Response::builder()
        .status(status)
        .header(ACCEPT_RANGES, "bytes")
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(CACHE_CONTROL, "private, max-age=3600")
        .header(CONTENT_LENGTH, content_length.to_string())
        .header(CONTENT_TYPE, mime);

    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(CONTENT_RANGE, format!("bytes {start}-{end}/{file_length}"));
    }

    Ok(builder.body(body_stream.boxed()).unwrap())
}

// -------------------------------------------------------------------------------------------------
// Test helpers / fakes
// -------------------------------------------------------------------------------------------------

/// In-memory [`StreamReader`] for unit and Range integration tests.
pub struct BytesStreamReader {
    name: String,
    data: Arc<Vec<u8>>,
    closed: AtomicBool,
}

impl BytesStreamReader {
    pub fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            data: Arc::new(data.into()),
            closed: AtomicBool::new(false),
        })
    }
}

impl StreamReader for BytesStreamReader {
    fn length(&self) -> i64 {
        self.data.len() as i64
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn read_at(&self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream reader closed",
            ));
        }
        if offset < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "negative offset",
            ));
        }
        let off = offset as usize;
        if off >= self.data.len() || buf.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - off);
        buf[..n].copy_from_slice(&self.data[off..off + n]);
        Ok(n)
    }
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

pub struct MapStreamSource {
    pub files: std::collections::HashMap<(String, usize), Arc<dyn StreamReader>>,
}

impl StreamSource for MapStreamSource {
    fn open(&self, id: &str, file_index: usize) -> io::Result<Arc<dyn StreamReader>> {
        self.files
            .get(&(id.to_string(), file_index))
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "torrent or file not found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn parse_stream_path_go_scheme() {
        assert_eq!(parse_stream_path("/torrent/abc/file/0"), Some(("abc", 0)));
        assert_eq!(parse_stream_path("/torrent/abc/0"), None);
        assert_eq!(parse_stream_path("/other/abc/file/0"), None);
    }

    #[test]
    fn range_start_matches_go() {
        assert_eq!(range_start("bytes=0-"), 0);
        assert_eq!(range_start("bytes=1048576-"), 1 << 20);
        assert_eq!(range_start("bytes=10-20"), 10);
        assert_eq!(range_start(" bytes=7- "), 7);
        assert_eq!(range_start(""), -1);
        assert_eq!(range_start("bytes=-500"), -1);
        assert_eq!(range_start("bytes=1-2,5-6"), -1);
        assert_eq!(range_start("items=1-"), -1);
        assert_eq!(range_start("bytes=x-"), -1);
    }

    #[test]
    fn parse_range_single_and_suffix() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=50-", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert!(parse_range("bytes=100-", 100).is_none());
        assert!(parse_range("bytes=0-1,2-3", 100).is_none());
    }

    #[test]
    fn content_type_video_and_fallback() {
        assert_eq!(content_type_for_name("movie.mp4"), "video/mp4");
        assert_eq!(content_type_for_name("show.mkv"), "video/x-matroska");
        assert_eq!(content_type_for_name("track.mp3"), "audio/mpeg");
        assert_eq!(
            content_type_for_name("notes.txt"),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn range_request_returns_206_and_bytes() {
        let payload: Vec<u8> = (0u8..200).collect();
        let reader = BytesStreamReader::new("movie.mp4", payload.clone());
        let mut files = std::collections::HashMap::new();
        files.insert(("t1".to_string(), 0usize), reader as Arc<dyn StreamReader>);
        let source: Arc<dyn StreamSource> = Arc::new(MapStreamSource { files });
        let server = start_stream_server(0, source).await.unwrap();
        let port = server.port();

        let url = format!("http://127.0.0.1:{port}/torrent/t1/file/0");
        let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http();
        let req = Request::builder()
            .method("GET")
            .uri(&url)
            .header(RANGE, "bytes=10-19")
            .body(Full::<Bytes>::new(Bytes::new()))
            .unwrap();
        let res = client.request(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            res.headers().get(CONTENT_RANGE).unwrap().to_str().unwrap(),
            "bytes 10-19/200"
        );
        assert_eq!(
            res.headers().get(ACCEPT_RANGES).unwrap().to_str().unwrap(),
            "bytes"
        );
        assert_eq!(
            res.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap(),
            "video/mp4"
        );
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], &payload[10..20]);

        // HEAD
        let req = Request::builder()
            .method("HEAD")
            .uri(&url)
            .header(RANGE, "bytes=0-3")
            .body(Full::<Bytes>::new(Bytes::new()))
            .unwrap();
        let res = client.request(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            res.headers().get(CONTENT_LENGTH).unwrap().to_str().unwrap(),
            "4"
        );
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty());

        // Bad range → 416
        let req = Request::builder()
            .method("GET")
            .uri(&url)
            .header(RANGE, "bytes=500-600")
            .body(Full::<Bytes>::new(Bytes::new()))
            .unwrap();
        let res = client.request(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        server.close();
    }
}

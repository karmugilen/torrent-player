//! Webseed peer implementation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::Rng;
use wt_discovery::webseed::{
    Extent, WebSeedClient, WebSeedError, DEFAULT_MAX_CONCURRENT as DISC_MAX,
};
use wt_types::{Info, PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};

use crate::CancelToken;

/// Default per-URL concurrent HTTP requests (matches discovery / Go MaxRequests=2
/// at torrent add, but discovery client default is 4).
pub const DEFAULT_MAX_CONCURRENT: usize = DISC_MAX;

/// Go `webseedHostRequestConcurrency` default (env override in Go).
pub const HOST_REQUEST_CONCURRENCY: usize = 25;

/// Go `webseed.MaxDiscardBytes` (48 KiB).
pub const MAX_DISCARD_BYTES: u64 = wt_discovery::webseed::MAX_DISCARD_BYTES;

/// Default max random error backoff (Go `rand.Int63n(10 * time.Second)`).
pub const DEFAULT_ERROR_BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Convict suspension after bad data (Go `providedBadData` → 1 minute).
pub const BAD_DATA_PENANCE: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct WebseedPeerConfig {
    pub url: String,
    pub max_concurrent: usize,
    pub error_backoff_max: Duration,
}

impl Default for WebseedPeerConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            error_backoff_max: DEFAULT_ERROR_BACKOFF_MAX,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WebseedPeerError {
    #[error(transparent)]
    WebSeed(#[from] WebSeedError),
    #[error("suspended until {0:?}")]
    Suspended(Duration),
    #[error("cancelled")]
    Cancelled,
    #[error("closed")]
    Closed,
    #[error("info not set")]
    NoInfo,
    #[error("{0}")]
    Other(String),
}

/// One 16 KiB (or tail) chunk delivered from a webseed response.
#[derive(Clone, Debug)]
pub struct WebseedChunk {
    pub request_index: RequestIndex,
    pub data: bytes::Bytes,
}

/// Reason string for immediate webseed replan (Go updateRequestReason).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeReason(pub &'static str);

struct CrimeState {
    last_crime: Option<String>,
    penance_complete: Instant,
}

/// BEP 19 web seed acting as a peer.
pub struct WebseedPeer {
    url: String,
    client: Mutex<WebSeedClient>,
    info: Mutex<Option<Info>>,
    crime: Mutex<CrimeState>,
    error_backoff_max: Duration,
    /// Pieces this seed can serve (all pieces once info is set, like Go).
    pieces_cardinality: AtomicUsize,
    num_pieces: AtomicUsize,
    active_requests: AtomicUsize,
    closed: Mutex<bool>,
}

impl WebseedPeer {
    pub fn new(cfg: WebseedPeerConfig) -> Result<Self, WebseedPeerError> {
        let client = WebSeedClient::new(&cfg.url, cfg.max_concurrent.max(1))?;
        Ok(Self {
            url: cfg.url,
            client: Mutex::new(client),
            info: Mutex::new(None),
            crime: Mutex::new(CrimeState {
                last_crime: None,
                penance_complete: Instant::now(),
            }),
            error_backoff_max: cfg.error_backoff_max,
            pieces_cardinality: AtomicUsize::new(0),
            num_pieces: AtomicUsize::new(0),
            active_requests: AtomicUsize::new(0),
            closed: Mutex::new(false),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn set_info(&self, info: Info) {
        let n = info.num_pieces() as usize;
        self.num_pieces.store(n, Ordering::SeqCst);
        self.pieces_cardinality.store(n, Ordering::SeqCst);
        self.client.lock().set_info(info.clone());
        *self.info.lock() = Some(info);
    }

    pub fn suspended(&self) -> bool {
        let c = self.crime.lock();
        c.last_crime.is_some() && Instant::now() < c.penance_complete
    }

    pub fn convict(&self, err: impl std::fmt::Display, term: Duration) {
        let mut c = self.crime.lock();
        if c.last_crime.is_some() && Instant::now() < c.penance_complete {
            return;
        }
        c.last_crime = Some(err.to_string());
        c.penance_complete = Instant::now() + term;
    }

    pub fn provided_bad_data(&self) {
        self.convict("provided bad data", BAD_DATA_PENANCE);
    }

    pub fn last_crime(&self) -> Option<String> {
        self.crime.lock().last_crime.clone()
    }

    pub fn active_request_count(&self) -> usize {
        self.active_requests.load(Ordering::SeqCst)
    }

    pub fn peer_has_all_pieces(&self) -> (bool, bool) {
        let n = self.num_pieces.load(Ordering::SeqCst);
        if n == 0 {
            return (true, false);
        }
        let card = self.pieces_cardinality.load(Ordering::SeqCst);
        (card == n, true)
    }

    pub fn connection_flags(&self) -> &'static str {
        "WS"
    }

    pub fn close(&self) {
        *self.closed.lock() = true;
    }

    /// Fetch `[begin, end)` request indexes as successive chunks.
    ///
    /// `request_span` returns `(torrent_byte_offset, chunk_length)` for a request index.
    /// `want_chunk` mirrors Go `wantReceiveChunk`; when no wanted chunk remains in the
    /// discard window the remaining body is still drained up to `MAX_DISCARD_BYTES`.
    pub async fn fetch_request_range<F, W>(
        &self,
        begin: RequestIndex,
        end: RequestIndex,
        mut request_span: F,
        want_chunk: W,
        cancel: CancelToken,
    ) -> Result<Vec<WebseedChunk>, WebseedPeerError>
    where
        F: FnMut(RequestIndex) -> (i64, u32),
        W: Fn(RequestIndex) -> bool,
    {
        if *self.closed.lock() {
            return Err(WebseedPeerError::Closed);
        }
        if self.suspended() {
            let left = self
                .crime
                .lock()
                .penance_complete
                .saturating_duration_since(Instant::now());
            return Err(WebseedPeerError::Suspended(left));
        }
        if begin >= end {
            return Ok(Vec::new());
        }
        if self.info.lock().is_none() {
            return Err(WebseedPeerError::NoInfo);
        }
        if cancel.is_cancelled() {
            return Err(WebseedPeerError::Cancelled);
        }

        let (start_off, _) = request_span(begin);
        let (last_off, last_len) = request_span(end - 1);
        let extent = Extent {
            start: start_off,
            length: (last_off + i64::from(last_len)) - start_off,
        };

        self.active_requests.fetch_add(1, Ordering::SeqCst);
        let fetch_result = {
            let client = self.client.lock().clone();
            client.fetch_extent(extent).await
        };
        let body = match fetch_result {
            Ok(b) => b,
            Err(e) => {
                self.active_requests.fetch_sub(1, Ordering::SeqCst);
                if !cancel.is_cancelled() {
                    self.convict(&e, BAD_DATA_PENANCE);
                    self.sleep_error_backoff().await;
                }
                return Err(e.into());
            }
        };

        let mut out = Vec::new();
        let mut offset = 0usize;
        let mut next = begin;
        let max_discard_chunks = (MAX_DISCARD_BYTES / u64::from(BLOCK_SIZE)).max(1) as RequestIndex;

        while next < end {
            if cancel.is_cancelled() {
                self.active_requests.fetch_sub(1, Ordering::SeqCst);
                return Err(WebseedPeerError::Cancelled);
            }
            let (_, chunk_len) = request_span(next);
            let chunk_len = chunk_len as usize;
            if offset + chunk_len > body.len() {
                self.active_requests.fetch_sub(1, Ordering::SeqCst);
                let err = WebseedPeerError::Other(format!(
                    "short webseed body at request {next}: need {} have {}",
                    offset + chunk_len,
                    body.len()
                ));
                self.convict(&err, BAD_DATA_PENANCE);
                return Err(err);
            }
            let data = body.slice(offset..offset + chunk_len);
            offset += chunk_len;
            out.push(WebseedChunk {
                request_index: next,
                data,
            });
            next += 1;

            if next < end && !wanted_in_discard_window(next, end, max_discard_chunks, &want_chunk) {
                // Go cancels the stream but keeps reading buffered body within discard.
                break;
            }
        }

        self.active_requests.fetch_sub(1, Ordering::SeqCst);
        Ok(out)
    }

    async fn sleep_error_backoff(&self) {
        let max = self.error_backoff_max.as_nanos().max(1) as u64;
        let nanos = rand::thread_rng().gen_range(0..max);
        tokio::time::sleep(Duration::from_nanos(nanos)).await;
    }
}

fn wanted_in_discard_window<W: Fn(RequestIndex) -> bool>(
    next: RequestIndex,
    end: RequestIndex,
    max_discard: RequestIndex,
    want: &W,
) -> bool {
    let window_end = next.saturating_add(max_discard);
    let mut ri = next;
    while ri < end && ri <= window_end {
        if want(ri) {
            return true;
        }
        ri += 1;
    }
    false
}

/// Map a half-open request-index range to a torrent-byte [`Extent`].
pub fn request_index_to_extent(
    begin: RequestIndex,
    end: RequestIndex,
    request_begin_byte: impl Fn(RequestIndex) -> i64,
    request_end_byte: impl Fn(RequestIndex) -> i64,
) -> Extent {
    let start = request_begin_byte(begin);
    let end_off = request_end_byte(end - 1);
    Extent {
        start,
        length: end_off - start,
    }
}

/// Webtor `webtorWakeWebseedsForPiece`: return a wake reason when an urgent
/// incomplete piece should force immediate webseed replan.
pub fn wake_for_urgent_piece(
    has_webseeds: bool,
    closed: bool,
    data_download_disallowed: bool,
    piece_complete: bool,
    pure_priority: PiecePriority,
) -> Option<WakeReason> {
    if !has_webseeds || closed || data_download_disallowed {
        return None;
    }
    if piece_complete || pure_priority < PiecePriority::Now {
        return None;
    }
    Some(WakeReason("webtor urgent piece"))
}

impl WebseedPeer {
    /// Instance helper wrapping [`wake_for_urgent_piece`].
    pub fn wake_for_urgent_piece(
        &self,
        _piece: PieceIndex,
        pure_priority: PiecePriority,
        piece_complete: bool,
    ) -> Option<WakeReason> {
        wake_for_urgent_piece(
            true,
            *self.closed.lock(),
            false,
            piece_complete,
            pure_priority,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wt_types::FileInfo;

    fn multi_file_info() -> Info {
        Info {
            name: "dir".into(),
            piece_length: 32768,
            pieces: vec![0u8; 40], // 2 pieces
            files: vec![
                FileInfo {
                    path: vec!["a.bin".into()],
                    length: 100,
                    torrent_offset: 0,
                    ..Default::default()
                },
                FileInfo {
                    path: vec!["b.bin".into()],
                    length: 100,
                    torrent_offset: 100,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    async fn spawn_range_server(payload: Arc<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let range = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split_once('='))
                    .map(|(_, r)| r.trim().to_string());
                let path_is_a = req.contains("/dir/a.bin");
                let path_is_b = req.contains("/dir/b.bin");
                let (file_off, file_len) = if path_is_a {
                    (0usize, 100usize)
                } else if path_is_b {
                    (100usize, 100usize)
                } else {
                    sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .ok();
                    continue;
                };
                let file_data = &payload[file_off..file_off + file_len];
                let (status, body) = if let Some(r) = range {
                    let r = r.trim_start_matches("bytes=");
                    let mut parts = r.split('-');
                    let start: usize = parts.next().unwrap().parse().unwrap();
                    let end: usize = parts.next().unwrap().parse().unwrap();
                    ("206 Partial Content", file_data[start..=end].to_vec())
                } else {
                    ("200 OK", file_data.to_vec())
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                sock.write_all(header.as_bytes()).await.ok();
                sock.write_all(&body).await.ok();
            }
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn fetch_chunks_across_files() {
        let payload: Arc<Vec<u8>> = Arc::new((0u8..200).collect());
        let base = spawn_range_server(Arc::clone(&payload)).await;
        let peer = WebseedPeer::new(WebseedPeerConfig {
            url: base,
            max_concurrent: 2,
            ..Default::default()
        })
        .unwrap();
        peer.set_info(multi_file_info());

        // Treat each byte as its own "chunk" for a tiny fixture (not 16 KiB).
        let begin = 90u32;
        let end = 110u32;
        let chunks = peer
            .fetch_request_range(
                begin,
                end,
                |ri| (i64::from(ri), 1),
                |_| true,
                CancelToken::default(),
            )
            .await
            .unwrap();
        assert_eq!(chunks.len(), 20);
        let got: Vec<u8> = chunks.iter().flat_map(|c| c.data.to_vec()).collect();
        assert_eq!(got, (90u8..110).collect::<Vec<_>>());
    }

    #[test]
    fn urgent_wake_only_for_now_incomplete() {
        assert!(wake_for_urgent_piece(true, false, false, false, PiecePriority::Now).is_some());
        assert!(wake_for_urgent_piece(true, false, false, true, PiecePriority::Now).is_none());
        assert!(wake_for_urgent_piece(true, false, false, false, PiecePriority::High).is_none());
        assert!(wake_for_urgent_piece(false, false, false, false, PiecePriority::Now).is_none());
    }

    #[test]
    fn convict_suspends() {
        let peer = WebseedPeer::new(WebseedPeerConfig {
            url: "http://example/".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(!peer.suspended());
        peer.provided_bad_data();
        assert!(peer.suspended());
        assert_eq!(peer.connection_flags(), "WS");
    }
}

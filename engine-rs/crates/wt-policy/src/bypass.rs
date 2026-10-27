//! Anti-DPI TCP write-split adapter (`engine-go/bypass.go`).
//!
//! [`BypassStream`] wraps any `AsyncRead + AsyncWrite` and fragments the first
//! [`crate::constants::BYPASS_SPLIT_FIRST_BYTES`] of outbound payload one byte
//! at a time (with an optional delay). The engine enables this via
//! [`BypassController`] and wraps newly dialed TCP peers / tracker sockets.
//! Loopback addresses should be left untreated ([`is_loopback_addr`]).

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::constants::{BYPASS_SPLIT_DELAY, BYPASS_SPLIT_FIRST_BYTES};

pub const BYPASS_MODE_OFF: &str = "off";
pub const BYPASS_MODE_TCP_SPLIT: &str = "tcp-write-split";

/// Transports adapted by the active technique.
pub fn bypass_tcp_coverage() -> Vec<String> {
    vec![
        "outgoing-tcp-peers".into(),
        "http-trackers".into(),
        "websocket-trackers".into(),
    ]
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BypassStatus {
    pub enabled: bool,
    pub active: bool,
    pub mode: String,
    pub coverage: Vec<String>,
    pub error: Option<String>,
}

/// Enable/disable state the engine mirrors into dial hooks.
#[derive(Debug, Default)]
pub struct BypassController {
    enabled: bool,
    active: bool,
    mode: String,
    coverage: Vec<String>,
    error: Option<String>,
    epoch: AtomicU64,
}

impl BypassController {
    pub fn snapshot(&self) -> BypassStatus {
        BypassStatus {
            enabled: self.enabled,
            active: self.active,
            mode: if self.mode.is_empty() {
                BYPASS_MODE_OFF.into()
            } else {
                self.mode.clone()
            },
            coverage: self.coverage.clone(),
            error: self.error.clone(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Activate split mode. `client_ok` is false when the torrent client is missing.
    pub fn set_enabled(&mut self, enabled: bool, client_ok: bool) -> BypassStatus {
        self.enabled = enabled;
        if !enabled {
            self.deactivate();
            return self.snapshot();
        }
        if !client_ok {
            self.error = Some("torrent client unavailable".into());
            self.active = false;
            self.mode = BYPASS_MODE_OFF.into();
            self.coverage.clear();
            return self.snapshot();
        }
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.active = true;
        self.mode = BYPASS_MODE_TCP_SPLIT.into();
        self.coverage = bypass_tcp_coverage();
        self.error = None;
        self.snapshot()
    }

    pub fn deactivate(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.active = false;
        self.mode = BYPASS_MODE_OFF.into();
        self.coverage.clear();
        self.error = None;
    }

    /// Shared flag so wrapped streams can stop splitting after deactivate.
    pub fn policy_flag(
        epoch: Arc<AtomicU64>,
        expected: u64,
    ) -> Arc<dyn Fn() -> bool + Send + Sync> {
        Arc::new(move || epoch.load(Ordering::SeqCst) == expected)
    }
}

/// True for localhost / loopback dial targets that must stay untreated.
pub fn is_loopback_addr(address: &str) -> bool {
    let host = match address.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h.trim_matches(['[', ']']),
        _ => address,
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

pin_project! {
    /// Tokio stream adapter that write-splits the first few outbound bytes.
    pub struct BypassStream<S> {
        #[pin]
        inner: S,
        remaining: usize,
        delay: Duration,
        disabled: AtomicBool,
        // When false, stop splitting (epoch mismatch / policy off).
        policy_active: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        // Injected sleep for tests; production uses tokio sleep via poll path.
        delay_fn: Option<Arc<dyn Fn(Duration) + Send + Sync>>,
        pending_delay: bool,
    }
}

impl<S> BypassStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            remaining: BYPASS_SPLIT_FIRST_BYTES,
            delay: BYPASS_SPLIT_DELAY,
            disabled: AtomicBool::new(false),
            policy_active: None,
            delay_fn: None,
            pending_delay: false,
        }
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn with_policy(mut self, policy: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.policy_active = Some(policy);
        self
    }

    /// Test hook: replace `sleep` with a no-op or counter.
    pub fn with_delay_fn(mut self, f: Arc<dyn Fn(Duration) + Send + Sync>) -> Self {
        self.delay_fn = Some(f);
        self
    }

    pub fn disable_split(&self) {
        self.disabled.store(true, Ordering::SeqCst);
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for BypassStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for BypassStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();
        if !this.disabled.load(Ordering::SeqCst) {
            if let Some(p) = this.policy_active {
                if !p() {
                    *this.remaining = 0;
                }
            }
        }
        if this.disabled.load(Ordering::SeqCst)
            || this.policy_active.as_ref().map(|p| !p()).unwrap_or(false)
            || *this.remaining == 0
            || buf.len() <= 1
        {
            return this.inner.poll_write(cx, buf);
        }

        // Fragment: write one byte (or remaining) per poll cycle.
        if *this.pending_delay {
            *this.pending_delay = false;
            if let Some(f) = this.delay_fn {
                f(*this.delay);
            } else if !this.delay.is_zero() {
                // Cooperative: yield once so the runtime can interleave.
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }

        let n = 1.min(*this.remaining).min(buf.len());
        match this.inner.as_mut().poll_write(cx, &buf[..n]) {
            Poll::Ready(Ok(0)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "tcp-write-split: zero-progress write",
            ))),
            Poll::Ready(Ok(wrote)) => {
                if *this.remaining > 0 {
                    *this.remaining = this.remaining.saturating_sub(wrote);
                }
                if *this.remaining > 0 && wrote < buf.len() && !this.delay.is_zero() {
                    *this.pending_delay = true;
                }
                Poll::Ready(Ok(wrote))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

/// Synchronous helper used by unit tests (mirrors Go `splitConn.Write`).
pub fn split_write_sync(
    remaining: &mut usize,
    delay: Duration,
    mut write: impl FnMut(&[u8]) -> io::Result<usize>,
    mut sleep: impl FnMut(Duration),
    policy_ok: impl Fn() -> bool,
    buf: &[u8],
) -> io::Result<usize> {
    if !policy_ok() || *remaining == 0 || buf.len() <= 1 {
        return write(buf);
    }
    let mut total = 0;
    let mut b = buf;
    while !b.is_empty() {
        if !policy_ok() {
            *remaining = 0;
        }
        let mut n = 1;
        if *remaining == 0 {
            n = b.len();
        }
        if *remaining > 0 && n > *remaining {
            n = *remaining;
        }
        let wrote = write(&b[..n])?;
        total += wrote;
        if wrote > 0 && *remaining > 0 {
            *remaining = remaining.saturating_sub(wrote);
        }
        if wrote == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "tcp-write-split: zero-progress write",
            ));
        }
        b = &b[wrote..];
        if *remaining > 0 && !b.is_empty() && !delay.is_zero() {
            sleep(delay);
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_bypass_defaults_off() {
        let c = BypassController::default();
        let s = c.snapshot();
        assert!(!s.enabled && !s.active);
        assert_eq!(s.mode, BYPASS_MODE_OFF);
        assert!(s.coverage.is_empty());
        assert!(s.error.is_none());
    }

    #[test]
    fn test_bypass_enable_activates_tcp_split() {
        let mut c = BypassController::default();
        let s = c.set_enabled(true, true);
        assert!(s.enabled && s.active);
        assert_eq!(s.mode, BYPASS_MODE_TCP_SPLIT);
        assert!(s.coverage.iter().any(|x| x == "outgoing-tcp-peers"));
        let s = c.set_enabled(false, true);
        assert!(!s.enabled && !s.active);
        assert_eq!(s.mode, BYPASS_MODE_OFF);
    }

    #[test]
    fn test_split_conn_fragments_initial_write() {
        let mut remaining = BYPASS_SPLIT_FIRST_BYTES;
        let mut writes: Vec<Vec<u8>> = Vec::new();
        let payload = b"BitTorrent protocolXXXXXXXX";
        split_write_sync(
            &mut remaining,
            Duration::ZERO,
            |b| {
                writes.push(b.to_vec());
                Ok(b.len())
            },
            |_| {},
            || true,
            payload,
        )
        .unwrap();
        assert!(writes.len() >= 2);
        assert_eq!(writes[0].len(), 1);
        let joined: Vec<u8> = writes.into_iter().flatten().collect();
        assert_eq!(joined, payload);
    }

    #[test]
    fn test_split_conn_zero_progress_does_not_spin() {
        let mut remaining = BYPASS_SPLIT_FIRST_BYTES;
        let err = split_write_sync(
            &mut remaining,
            Duration::ZERO,
            |_| Ok(0),
            |_| {},
            || true,
            b"hello-world",
        )
        .unwrap_err();
        assert!(err.to_string().contains("zero-progress"));
    }

    #[test]
    fn test_bypass_uses_per_client_hook() {
        // Controllers are independent; enabling one must not activate another.
        let mut a = BypassController::default();
        let b = BypassController::default();
        let _ = a.set_enabled(true, true);
        let status = b.snapshot();
        assert!(!status.enabled && !status.active);
        let _ = a.set_enabled(false, true);
        assert!(!a.snapshot().active);
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_addr("127.0.0.1:80"));
        assert!(is_loopback_addr("localhost:6881"));
        assert!(is_loopback_addr("[::1]:80"));
        assert!(!is_loopback_addr("1.1.1.1:80"));
    }

    #[tokio::test]
    async fn bypass_stream_async_roundtrip() {
        let (client, mut server) = duplex(1024);
        let mut wrapped = BypassStream::new(client).with_delay(Duration::ZERO);
        let payload = b"BitTorrent protocolXXXXXXXX";
        wrapped.write_all(payload).await.unwrap();
        wrapped.flush().await.unwrap();
        let mut buf = vec![0u8; payload.len()];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, payload);
    }
}

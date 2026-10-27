//! Unified tracker client factory (HTTP + UDP).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::net::UdpSocket;
use url::Url;

use super::http::{HttpClient, HttpClientOpts};
use super::udp::{UdpClient, UdpClientOpts, DEFAULT_TRACKER_ANNOUNCE_TIMEOUT};
use super::{AnnounceOpt, AnnounceRequest, AnnounceResponse, Result, ScrapeResponse, TrackerError};
use crate::datagram::DatagramIo;

type DialFut = Pin<Box<dyn Future<Output = std::io::Result<tokio::net::TcpStream>> + Send>>;
type ListenFut = Pin<Box<dyn Future<Output = std::io::Result<UdpSocket>> + Send>>;

/// Custom TCP dialer hook so the engine can route / track sockets.
///
/// Signature mirrors Go `DialContext(ctx, network, addr) (Conn, error)` but
/// returns a connected `TcpStream`. Wired through HTTP(S) tracker clients
/// (and WebSocket trackers via `webtorrent::TrackerClientConfig::dialer`).
pub type DialerHook = Arc<dyn Fn(String, String) -> DialFut + Send + Sync>;

/// Custom UDP bind hook (Go `ListenPacket`).
pub type ListenPacketHook = Arc<dyn Fn(&str, &str) -> ListenFut + Send + Sync>;

#[derive(Clone, Default)]
pub struct ClientOpts {
    pub http: HttpClientOpts,
    pub udp_network: String,
    pub listen_packet: Option<ListenPacketHook>,
    /// Shared UDP IO for UDP trackers (wt-wire non-uTP demux).
    pub datagram: Option<Arc<dyn DatagramIo>>,
    /// Top-level dialer; copied into `http.dialer` when that field is empty.
    pub dialer: Option<DialerHook>,
}

/// Tracker client for a single announce URL.
pub enum Client {
    Http(HttpClient),
    Udp(UdpClient),
}

impl Client {
    pub async fn new(url_str: &str, opts: ClientOpts) -> Result<Self> {
        let url = Url::parse(url_str).map_err(|e| TrackerError::Other(e.to_string()))?;
        match url.scheme() {
            "http" | "https" => {
                let mut http_opts = opts.http;
                if http_opts.timeout.is_none() {
                    http_opts.timeout = Some(DEFAULT_TRACKER_ANNOUNCE_TIMEOUT);
                }
                if http_opts.dialer.is_none() {
                    http_opts.dialer = opts.dialer;
                }
                Ok(Self::Http(HttpClient::new(url, http_opts)?))
            }
            "udp" | "udp4" | "udp6" => {
                let network = if opts.udp_network.is_empty() {
                    url.scheme().to_string()
                } else {
                    opts.udp_network
                };
                let host = url
                    .host_str()
                    .ok_or_else(|| TrackerError::Other("udp url missing host".into()))?;
                let port = url.port().unwrap_or(80);
                let host_port = if host.contains(':') && !host.starts_with('[') {
                    // already unusual
                    format!("{host}:{port}")
                } else if url.scheme() == "udp6" || host.contains(':') {
                    format!("[{host}]:{port}")
                } else {
                    format!("{host}:{port}")
                };
                let request_uri = {
                    let path = url.path();
                    match url.query() {
                        Some(q) => format!("{path}?{q}"),
                        None => path.to_string(),
                    }
                };
                let udp = UdpClient::new(
                    &host_port,
                    &request_uri,
                    UdpClientOpts {
                        network,
                        listen_packet: opts.listen_packet,
                        datagram: opts.datagram,
                        timeout: Some(DEFAULT_TRACKER_ANNOUNCE_TIMEOUT),
                    },
                )
                .await?;
                Ok(Self::Udp(udp))
            }
            _ => Err(TrackerError::BadScheme),
        }
    }

    pub async fn announce(
        &self,
        req: AnnounceRequest,
        opt: AnnounceOpt,
    ) -> Result<AnnounceResponse> {
        match self {
            Self::Http(c) => c.announce(req, opt).await,
            Self::Udp(c) => c.announce(req, opt).await,
        }
    }

    pub async fn scrape(&self, info_hashes: &[[u8; 20]]) -> Result<ScrapeResponse> {
        match self {
            Self::Http(c) => c.scrape(info_hashes).await,
            Self::Udp(c) => c.scrape(info_hashes).await,
        }
    }
}

/// One-shot announce (Go `tracker.Announce.Do`).
pub async fn announce(tracker_url: &str, req: AnnounceRequest) -> Result<AnnounceResponse> {
    announce_with_opts(
        tracker_url,
        req,
        AnnounceOpt::default(),
        ClientOpts::default(),
    )
    .await
}

pub async fn announce_with_opts(
    tracker_url: &str,
    req: AnnounceRequest,
    opt: AnnounceOpt,
    client_opts: ClientOpts,
) -> Result<AnnounceResponse> {
    let cl = Client::new(tracker_url, client_opts).await?;
    cl.announce(req, opt).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bad_scheme() {
        let err = match Client::new("lol://tracker.example/announce", ClientOpts::default()).await {
            Err(e) => e,
            Ok(_) => panic!("expected BadScheme"),
        };
        assert!(matches!(err, TrackerError::BadScheme));
    }
}

//! HTTP(S) and UDP (BEP 15) BitTorrent trackers + per-torrent announce scheduling.
//!
//! Ports `engine-go/patches/torrent/tracker/` and `client-tracker-announcer.go`.

mod announcer;
mod client;
mod http;
mod udp;

pub use announcer::{
    AnnounceDispatcher, AnnounceDispatcherConfig, AnnounceRequestFactory, TorrentAnnounceKey,
    TorrentAnnounceSnapshot, MAX_CONCURRENT_ANNOUNCES_PER_TRACKER,
};
pub use client::{announce, announce_with_opts, Client, ClientOpts, DialerHook, ListenPacketHook};
pub use http::{HttpClient, HttpClientOpts};
pub use udp::{udp_timeout, UdpClient, UdpClientOpts, DEFAULT_TRACKER_ANNOUNCE_TIMEOUT};

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use wt_types::{InfoHash, PeerId};

/// Announce event (BEP 3 / BEP 15). Numeric values match Go `shared.AnnounceEvent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(i32)]
pub enum AnnounceEvent {
    /// Regular interval announce (empty string in HTTP).
    #[default]
    None = 0,
    Completed = 1,
    Started = 2,
    Stopped = 3,
}

impl AnnounceEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Completed => "completed",
            Self::Started => "started",
            Self::Stopped => "stopped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "" | "none" | "<regular>" => Some(Self::None),
            "completed" => Some(Self::Completed),
            "started" => Some(Self::Started),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }
}

impl fmt::Display for AnnounceEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("<regular>"),
            other => f.write_str(other.as_str()),
        }
    }
}

/// Binary layout matches Go `udp.AnnounceRequest` (82 bytes, big-endian).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceRequest {
    pub info_hash: InfoHash,
    pub peer_id: PeerId,
    pub downloaded: i64,
    /// If negative, HTTP trackers send `math.MaxInt64`.
    pub left: i64,
    pub uploaded: i64,
    pub event: AnnounceEvent,
    pub ip_address: u32,
    pub key: i32,
    /// How many peers desired; `-1` for tracker default.
    pub num_want: i32,
    pub port: u16,
}

impl Default for AnnounceRequest {
    fn default() -> Self {
        Self {
            info_hash: InfoHash::default(),
            peer_id: [0u8; 20],
            downloaded: 0,
            left: 0,
            uploaded: 0,
            event: AnnounceEvent::None,
            ip_address: 0,
            key: 0,
            num_want: -1,
            port: 0,
        }
    }
}

/// Peer returned by a tracker.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Peer {
    pub addr: SocketAddr,
    pub id: Option<PeerId>,
}

impl Peer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr, id: None }
    }
}

/// Successful announce result (HTTP + UDP unified).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnounceResponse {
    /// Seconds until next announce (tracker `interval`).
    pub interval: i32,
    /// Optional BEP 3 `min interval`.
    pub min_interval: Option<i32>,
    pub leechers: i32,
    pub seeders: i32,
    pub peers: Vec<Peer>,
    /// Optional BEP 3 `warning message` (non-fatal).
    pub warning: Option<String>,
    pub tracker_id: Option<String>,
}

/// Optional HTTP announce parameters.
#[derive(Clone, Debug, Default)]
pub struct AnnounceOpt {
    pub user_agent: String,
    pub host_header: String,
    pub client_ip4: Option<IpAddr>,
    pub client_ip6: Option<IpAddr>,
}

/// Scrape row for one infohash (BEP 15 / HTTP scrape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrapeItem {
    pub complete: i32,
    pub downloaded: i32,
    pub incomplete: i32,
}

pub type ScrapeResponse = Vec<ScrapeItem>;

#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    #[error("unknown scheme")]
    BadScheme,
    #[error("tracker failure reason: {0}")]
    FailureReason(String),
    #[error("http status {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("timeout")]
    Timeout,
    #[error("udp protocol: {0}")]
    UdpProtocol(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, TrackerError>;

//! Standalone compile of `wt-core::sources` while the parent crate is mid-edit.

use std::sync::Arc;

pub use wt_types::*;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("closed")]
    Closed,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Clone, Debug)]
pub enum StatusEvent {
    TrackerConnected {
        url: String,
        info_hash: InfoHash,
    },
    TrackerDisconnected {
        url: String,
        info_hash: InfoHash,
        error: Option<String>,
    },
    TrackerAnnounceSuccessful {
        url: String,
        info_hash: InfoHash,
    },
    TrackerAnnounceError {
        url: String,
        info_hash: InfoHash,
        error: String,
    },
}

pub type StatusCallbacks = Vec<Arc<dyn Fn(&StatusEvent) + Send + Sync>>;

#[derive(Clone, Default)]
pub struct Callbacks {
    pub status_updated: StatusCallbacks,
}

/// Minimal stand-in for `wt_core::ClientConfig` fields sources reads.
#[derive(Clone)]
pub struct ClientConfig {
    pub listen_port: u16,
    pub no_dht: bool,
    pub disable_webtorrent: bool,
    pub periodically_announce_torrents_to_dht: bool,
    pub http_user_agent: String,
    pub ice_servers: Vec<String>,
    pub torrent_peers_high_water: usize,
    pub torrent_peers_low_water: usize,
    pub callbacks: Callbacks,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            listen_port: 0,
            no_dht: false,
            disable_webtorrent: false,
            periodically_announce_torrents_to_dht: false,
            http_user_agent: "Webtor/2.0".into(),
            ice_servers: Vec::new(),
            torrent_peers_high_water: 200,
            torrent_peers_low_water: 50,
            callbacks: Callbacks::default(),
        }
    }
}

#[derive(Clone, Default)]
pub struct TorrentSpec {
    pub info_hash: Option<InfoHash>,
    pub info_hash_v2: Option<InfoHashV2>,
    pub trackers: Vec<Vec<String>>,
    pub web_seeds: Vec<String>,
    pub dht_nodes: Vec<String>,
    pub display_name: String,
}

#[path = "../../../src/sources/mod.rs"]
pub mod sources;

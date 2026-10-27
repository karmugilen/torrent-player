//! Traits agy-2 implements so `Sources` never locks torrent internals.

use std::sync::Arc;

use wt_discovery::webtorrent::DataChannelStream;
use wt_types::{InfoHash, PeerId, PeerInfo};

/// Short infohashes a torrent is discoverable under.
///
/// Hybrid magnets must include both v1 and [`wt_types::InfoHashV2::to_short`]
/// so tracker/DHT keys match Go `torrentsByShortHash` (WEBTOR_PATCH).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TorrentKeys {
    pub v1: Option<InfoHash>,
    pub v2_short: Option<InfoHash>,
}

impl TorrentKeys {
    pub fn all_short(&self) -> Vec<InfoHash> {
        let mut out = Vec::with_capacity(2);
        if let Some(v) = self.v1 {
            out.push(v);
        }
        if let Some(v) = self.v2_short {
            if !out.contains(&v) {
                out.push(v);
            }
        }
        out
    }
}

/// Live announce stats for one torrent (agy-2 reads from `TorrentInner`).
pub trait TorrentAnnounceInfo: Send + Sync {
    fn peer_id(&self) -> PeerId;
    fn downloaded(&self) -> i64;
    fn uploaded(&self) -> i64;
    /// Bytes left; negative → HTTP trackers send MaxInt64 (BEP 3).
    fn left(&self) -> i64;
    fn port(&self) -> u16;
    fn announce_key(&self) -> i32;
    fn need_data(&self) -> bool;
    /// Typically `known_peers.len() <= torrent_peers_low_water`.
    fn want_peers(&self) -> bool;
    fn completed(&self) -> bool;
    /// Edge trigger for the Completed announce event.
    fn newly_completed(&self) -> bool;
}

/// Opened WebRTC data channel from a WebTorrent tracker.
pub struct WebRtcOpenedPeer {
    pub info_hash: InfoHash,
    pub peer_id: PeerId,
    pub offer_id: String,
    pub local_offered: bool,
    pub stream: DataChannelStream,
}

/// Sink for peers discovered by trackers / DHT / WebTorrent.
pub trait PeerSink: Send + Sync {
    fn add_peers(&self, info_hash: InfoHash, peers: Vec<PeerInfo>);

    fn add_webrtc_peer(&self, info_hash: InfoHash, peer: WebRtcOpenedPeer) {
        let _ = (info_hash, peer);
    }

    /// BEP 19 URLs; grok-6/`webseed_peer` dials them. Default no-op.
    fn add_web_seeds(&self, info_hash: InfoHash, urls: Vec<String>) {
        let _ = (info_hash, urls);
    }
}

/// Bundle passed to [`super::Sources::register_torrent`].
pub struct RegisterTorrent {
    pub keys: TorrentKeys,
    pub trackers: Vec<Vec<String>>,
    pub web_seeds: Vec<String>,
    pub info: Arc<dyn TorrentAnnounceInfo>,
    pub sink: Arc<dyn PeerSink>,
}

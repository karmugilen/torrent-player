//! BitTorrent client core: pure Rust replacement for the patched
//! `engine-go/patches/torrent` (anacrolix/torrent v1.61.0 + Webtor patches).

#![allow(clippy::type_complexity)]

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use wt_types::*;

pub mod choking;
pub mod client;
pub mod conn;
pub mod hashing;
pub mod metadata;
pub mod peer_conn;
pub mod pex;
pub mod picker;
pub mod piece_state;
pub mod ratelimit;
pub mod sched;
pub mod reader;
pub mod sources;
pub mod stats;
pub mod storage_adapter;
pub mod torrent;
pub mod torrent_impls;
pub mod tuning;
mod webseed_download;
pub mod webseed_peer;
pub mod wire;

pub use client::ClientInner;
pub use stats::{ClientStats, ConnStats, PeerConnInfo, TorrentStats};
pub use storage_adapter::{verify_piece_hash, MemoryClientStorage};
pub use torrent::TorrentInner;
pub use tuning::{current_seek_tuning, set_seek_tuning, SeekTuning};
pub use wt_wire::mse::HeaderObfuscationPolicy;

// ---------------------------------------------------------------- config

/// Mirrors the `torrent.ClientConfig` fields `engine-go/server.go` sets.
#[derive(Clone)]
pub struct ClientConfig {
    pub listen_port: u16,
    pub no_dht: bool,
    pub disable_utp: bool,
    pub disable_tcp: bool,
    pub disable_webtorrent: bool,
    pub disable_pex: bool,
    pub seed: bool,
    pub dial_for_peer_conns: bool,
    pub accept_peer_connections: bool,
    pub always_want_conns: bool,
    pub periodically_announce_torrents_to_dht: bool,
    pub piece_hashers_per_torrent: usize,
    /// Dials per second (token bucket rate == burst), `defaultDialRate` in Go.
    pub dial_rate: f64,
    /// Bytes/s; `None` = unlimited. Burst is 1 MiB like the Go limiters.
    pub download_rate: Option<f64>,
    pub upload_rate: Option<f64>,
    pub nominal_dial_timeout: Duration,
    pub min_dial_timeout: Duration,
    pub handshakes_timeout: Duration,
    pub established_conns_per_torrent: usize,
    pub half_open_conns_per_torrent: usize,
    pub total_half_open_conns: usize,
    pub torrent_peers_high_water: usize,
    pub torrent_peers_low_water: usize,
    pub http_user_agent: String,
    /// Map the listen port on the local router (UPnP, else NAT-PMP), like Go
    /// `forwardPort`. Off by default so tests never touch a real router.
    pub port_forwarding: bool,
    /// BEP 14 Local Service Discovery of peers on the local network.
    pub local_service_discovery: bool,
    /// Dial hook shared by HTTP, HTTPS and WebSocket trackers.
    pub tracker_dialer: Option<wt_discovery::tracker::DialerHook>,
    pub extended_handshake_client_version: String,
    pub ice_servers: Vec<String>,
    pub default_storage: Arc<dyn ClientStorage>,
    pub header_obfuscation_policy: HeaderObfuscationPolicy,
    pub callbacks: Callbacks,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            listen_port: 0,
            no_dht: false,
            disable_utp: false,
            disable_tcp: false,
            disable_webtorrent: false,
            disable_pex: false,
            seed: false,
            dial_for_peer_conns: true,
            accept_peer_connections: true,
            always_want_conns: false,
            periodically_announce_torrents_to_dht: false,
            piece_hashers_per_torrent: 2,
            dial_rate: 100.0,
            download_rate: None,
            upload_rate: None,
            nominal_dial_timeout: Duration::from_secs(10),
            min_dial_timeout: Duration::from_millis(500),
            handshakes_timeout: Duration::from_secs(4),
            established_conns_per_torrent: 80,
            half_open_conns_per_torrent: 20,
            total_half_open_conns: 100,
            torrent_peers_high_water: 200,
            torrent_peers_low_water: 50,
            http_user_agent: "Webtor/2.0".to_string(),
            port_forwarding: false,
            local_service_discovery: false,
            tracker_dialer: None,
            extended_handshake_client_version: "Webtor 2.0".to_string(),
            ice_servers: Vec::new(),
            default_storage: Arc::new(MemoryClientStorage::new()),
            header_obfuscation_policy: HeaderObfuscationPolicy::default(),
            callbacks: Callbacks::default(),
        }
    }
}

/// Library callbacks (`torrent.Callbacks`). Called without the client lock held.
#[derive(Clone, Default)]
pub struct Callbacks {
    pub status_updated: Vec<Arc<dyn Fn(&StatusEvent) + Send + Sync>>,
    pub completed_handshake: Vec<Arc<dyn Fn(&PeerConnInfo, InfoHash) + Send + Sync>>,
}

/// Tracker status events (`torrent.StatusUpdatedEvent`).
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

/// What a torrent is added from (`torrent.TorrentSpec`).
#[derive(Clone, Default)]
pub struct TorrentSpec {
    pub info_hash: Option<InfoHash>,
    pub info_hash_v2: Option<InfoHashV2>,
    pub info_bytes: Option<Vec<u8>>,
    pub trackers: Vec<Vec<String>>,
    pub display_name: String,
    pub web_seeds: Vec<String>,
    pub dht_nodes: Vec<String>,
    pub piece_layers: Vec<([u8; 32], Vec<u8>)>,
    /// Use this storage instead of `ClientConfig::default_storage`.
    pub storage: Option<Arc<dyn ClientStorage>>,
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("closed")]
    Closed,
    #[error("info not available yet")]
    NoInfo,
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;

// ---------------------------------------------------------------- client

#[derive(Clone)]
pub struct Client {
    pub inner: Arc<ClientInner>,
}

/// Wraps outgoing TCP peer streams (`Client.SetOutgoingTCPConnHook`, used by anti-DPI).
pub type TcpConnHook = Arc<dyn Fn(tokio::net::TcpStream) -> Box<dyn PeerStream> + Send + Sync>;

/// Any byte stream a peer connection can run on.
pub trait PeerStream:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static> PeerStream for T {}

/// Half-open dial budget derived from the established peer cap.
///
/// Matches the unlocked Bunny harness: raising maxPeers also raises pending
/// dials so a 128-peer cap is not stuck behind a hard-coded 24 half-open limit.
pub fn half_open_for_max_peers(max_peers: usize) -> (usize, usize) {
    let peers = max_peers.clamp(1, 500);
    let per_torrent = peers.max(24).min(500);
    let total = peers.saturating_mul(2).max(48).min(500);
    (per_torrent, total)
}

impl Client {
    pub fn new(cfg: ClientConfig) -> Result<Client> {
        let inner = ClientInner::new(cfg)?;
        Ok(Client { inner })
    }

    /// Update the live half-open dial budget (for example when maxPeers changes).
    pub fn set_half_open_limits(&self, per_torrent: usize, total: usize) {
        self.inner
            .half_open_conns_per_torrent
            .store(per_torrent.max(1), Ordering::Relaxed);
        self.inner
            .total_half_open_conns
            .store(total.max(1), Ordering::Relaxed);
    }

    /// Returns the torrent and whether it was newly added.
    pub fn add_torrent_spec(&self, spec: TorrentSpec) -> Result<(Torrent, bool)> {
        let (inner, newly) = self.inner.add_torrent_spec(spec)?;
        Ok((Torrent { inner }, newly))
    }

    pub fn torrent(&self, ih: InfoHash) -> Option<Torrent> {
        self.inner.torrent(ih).map(|inner| Torrent { inner })
    }

    pub fn torrents(&self) -> Vec<Torrent> {
        self.inner
            .torrents()
            .into_iter()
            .map(|inner| Torrent { inner })
            .collect()
    }

    pub fn local_port(&self) -> u16 {
        self.inner.local_port()
    }

    pub fn listen_addrs(&self) -> Vec<SocketAddr> {
        self.inner.listen_addrs()
    }

    pub fn add_dht_nodes(&self, nodes: &[String]) {
        self.inner.add_dht_nodes(nodes);
    }

    pub fn dht_contacts(&self) -> Vec<SocketAddr> {
        self.inner.dht_contacts()
    }

    /// Redo router port mapping and LSD announces after a network change.
    pub fn refresh_local_network(&self) {
        self.inner.refresh_local_network();
    }

    pub fn set_outgoing_tcp_conn_hook(&self, hook: Option<TcpConnHook>) {
        self.inner.set_outgoing_tcp_conn_hook(hook);
    }

    pub fn set_urgent_request_stealing(&self, after: Duration) {
        self.inner.set_urgent_request_stealing(after);
    }




    pub fn stats(&self) -> ClientStats {
        let mut cs = ConnStats::default();
        let torrents = self.inner.torrents.read();
        for t in torrents.values() {
            let conns = t.conn_handles.read();
            cs.add(&t.retired_conn_stats.lock());
            cs.add(&t.webseed_stats.snapshot());
            for c in conns.values() {
                cs.add(&c.stats().snapshot());
            }
        }
        ClientStats {
            conn_stats: cs,
            active_half_open: self.inner.half_open.current_total(),
        }
    }

    pub fn close(&self) {
        self.inner.close();
    }
}

// ---------------------------------------------------------------- torrent

#[derive(Clone)]
pub struct Torrent {
    pub inner: Arc<TorrentInner>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PieceState {
    pub priority: PiecePriority,
    pub complete: bool,
    pub checking: bool,
    pub partial: bool,
    pub ok: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PieceStateRun {
    pub state: PieceState,
    pub length: u32,
}

/// Forward playhead movement that is a jump rather than playback.
const FOCUS_WAKE_BYTES: i64 = 1 << 20;
const REFOCUS_GAP: Duration = Duration::from_millis(50);

impl Torrent {
    pub fn info_hash(&self) -> InfoHash {
        self.inner.info_hash
    }

    pub fn info_hash_v2(&self) -> Option<InfoHashV2> {
        self.inner.info_hash_v2
    }

    pub fn name(&self) -> String {
        self.inner.display_name.clone()
    }

    pub fn info(&self) -> Option<Arc<Info>> {
        self.inner.info.read().clone()
    }

    pub fn wait_info(&self, timeout: Option<Duration>) -> Result<Arc<Info>> {
        let deadline = timeout.map(|d| Instant::now() + d);
        loop {
            if self.inner.closed.load(Ordering::Relaxed) {
                return Err(CoreError::Closed);
            }
            if let Some(info) = self.inner.info.read().clone() {
                return Ok(info);
            }
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    return Err(CoreError::Other("timeout waiting for info".to_string()));
                }
            }
            let wait_dur = deadline
                .map(|dl| dl.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(50));
            self.inner.wait_version(
                self.inner.change_version.load(Ordering::SeqCst),
                wait_dur.min(Duration::from_millis(50)),
            );
        }
    }

    pub fn metainfo(&self) -> Result<MetaInfo> {
        self.inner
            .metainfo_raw
            .read()
            .clone()
            .ok_or(CoreError::NoInfo)
    }

    pub fn files(&self) -> Vec<File> {
        let Some(info) = self.inner.info.read().clone() else {
            return Vec::new();
        };
        let mut files = Vec::new();
        for (i, f) in info.files.iter().enumerate() {
            files.push(File {
                torrent: self.inner.clone(),
                file_index: i,
                path: if f.path.is_empty() {
                    info.name.clone()
                } else {
                    format!("{}/{}", info.name, f.path.join("/"))
                },
                offset: f.torrent_offset,
                length: f.length,
            });
        }
        files
    }

    pub fn num_pieces(&self) -> u32 {
        self.inner.pieces.read().len() as u32
    }

    pub fn set_piece_priority(&self, piece: PieceIndex, p: PiecePriority) {
        self.set_piece_priorities([(piece, p)]);
    }

    /// Publish a playback window with one lock and one swarm wakeup. Waking
    /// every connection once per piece stalls control requests on large files.
    pub fn set_piece_priorities(
        &self,
        priorities: impl IntoIterator<Item = (PieceIndex, PiecePriority)>,
    ) {
        let ranges: Vec<_> = self.inner.reader_ranges.read().values().copied().collect();
        let (now, readahead) = crate::reader::reader_piece_priorities(&ranges);
        let mut pieces = self.inner.pieces_mut();
        let mut changed = false;
        for (piece, p) in priorities {
            if let Some(pe) = pieces.get_mut(piece as usize) {
                let reader = if now.contains(&piece) {
                    PiecePriority::Now
                } else if readahead.contains(&piece) {
                    PiecePriority::High
                } else {
                    PiecePriority::None
                };
                let effective = p.max(reader);
                changed |= pe.priority != effective || pe.base_priority != p;
                pe.priority = effective;
                pe.base_priority = p;
            }
        }
        drop(pieces);
        if changed {
            self.inner.bump_change();
        }
    }

    pub fn piece_state(&self, piece: PieceIndex) -> PieceState {
        let pieces = self.inner.pieces.read();
        pieces
            .get(piece as usize)
            .map(|p| p.state())
            .unwrap_or_default()
    }

    pub fn piece_state_runs(&self) -> Vec<PieceStateRun> {
        let pieces = self.inner.pieces.read();
        let mut runs = Vec::new();
        if pieces.is_empty() {
            return runs;
        }

        let mut cur_state = pieces[0].state();
        let mut cur_len = 0;
        for p in pieces.iter() {
            let st = p.state();
            if st == cur_state {
                cur_len += 1;
            } else {
                runs.push(PieceStateRun {
                    state: cur_state,
                    length: cur_len,
                });
                cur_state = st;
                cur_len = 1;
            }
        }
        runs.push(PieceStateRun {
            state: cur_state,
            length: cur_len,
        });
        runs
    }

    pub fn piece_complete(&self, piece: PieceIndex) -> bool {
        let pieces = self.inner.pieces.read();
        pieces
            .get(piece as usize)
            .map(|p| p.complete)
            .unwrap_or(false)
    }

    pub fn update_piece_completion(&self, piece: PieceIndex) {
        if self.refresh_piece_completion(piece) {
            self.inner.bump_change();
        }
    }

    /// Refresh every piece with one scheduler wake instead of one per piece.
    pub fn update_all_piece_completion(&self) {
        let mut refreshed = false;
        for piece in 0..self.num_pieces() {
            refreshed |= self.refresh_piece_completion(piece);
        }
        if refreshed {
            self.inner.bump_change();
        }
    }

    fn refresh_piece_completion(&self, piece: PieceIndex) -> bool {
        let Some(storage) = self.inner.storage.read().clone() else {
            return false;
        };
        let Some(info) = self.inner.info.read().clone() else {
            return false;
        };
        let piece_obj = info.piece(piece);
        let piece_storage = storage.piece(piece_obj, info.piece_hash_v1(piece));
        let comp = piece_storage.completion();
        let complete = comp.ok && comp.complete;
        let (begin, end) = crate::picker::piece_req_index_range(
            piece,
            info.piece_length,
            info.total_length(),
        );
        // Bounded Direct Play RAM can drop a partial piece while dirty bits
        // still claim we have those blocks (Go errStreamEvicted: "must be
        // fetched again"). Probe storage: if we tracked local data but the
        // piece is no longer readable, treat it as an eviction.
        let had_local_data = {
            let pieces = self.inner.pieces.read();
            let p = pieces.get(piece as usize);
            let was_complete = p.map(|p| p.complete).unwrap_or(false);
            let was_partial = p
                .map(|p| p.partial)
                .unwrap_or(false);
            let has_dirty = self.inner.dirty_chunks.any_in_range(begin, end);
            was_complete || was_partial || has_dirty
        };
        let storage_lost = !complete
            && had_local_data
            && {
                let mut probe = [0u8];
                piece_storage.read_at(&mut probe, 0).is_err()
            };
        if storage_lost {
            // Direct Play marks an evicted partial piece as lost. Clearing
            // only the scheduler's dirty bits leaves that storage flag set:
            // every replacement block is then unreadable, including to the
            // hasher. Reset both sides before requesting replacement blocks.
            let _ = piece_storage.mark_not_complete();
        }
        let evicted = {
            let mut pieces = self.inner.pieces_mut();
            if let Some(p) = pieces.get_mut(piece as usize) {
                let evicted = (p.complete && !complete) || storage_lost;
                p.complete = complete;
                p.ok = comp.ok;
                if evicted {
                    p.partial = false;
                    p.checking = false;
                }
                evicted
            } else {
                false
            }
        };
        // Completion and dirty/request bitmaps must agree after bounded storage
        // discards a piece, otherwise the scheduler never requests it again.
        if evicted {
            self.inner.dirty_chunks.remove_range(begin, end);
        }
        true
    }

    pub fn bytes_completed(&self) -> i64 {
        let Some(info) = self.inner.info.read().clone() else {
            return 0;
        };
        let pieces = self.inner.pieces.read();
        let mut completed = 0;
        for p in pieces.iter() {
            if p.complete {
                completed += info.piece(p.index).length;
            }
        }
        completed
    }

    pub fn bytes_missing(&self) -> i64 {
        let Some(info) = self.inner.info.read().clone() else {
            return 0;
        };
        info.total_length() - self.bytes_completed()
    }

    pub fn stats(&self) -> TorrentStats {
        let mut cs = self.inner.webseed_stats.snapshot();
        let handles = self.inner.conn_handles.read();
        cs.add(&self.inner.retired_conn_stats.lock());
        let snapshots = self.inner.peer_snapshots.read();
        let dialing = self.inner.dialing_peers.lock();
        let pending = self
            .inner
            .known_peers
            .read()
            .iter()
            .filter(|peer| !handles.contains_key(&peer.addr) && !dialing.contains(&peer.addr))
            .map(|peer| peer.addr)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let half_open = dialing.len();
        drop(dialing);
        let (total, seeders) = if !handles.is_empty() || !snapshots.is_empty() {
            for h in handles.values() {
                cs.add(&h.stats().snapshot());
            }
            (
                handles.len(),
                snapshots
                    .values()
                    .filter(|s| s.peer_completion >= 1.0)
                    .count(),
            )
        } else {
            (0, 0)
        };
        drop(snapshots);
        drop(handles);
        TorrentStats {
            conn_stats: cs,
            total_peers: total,
            pending_peers: pending,
            active_peers: total,
            connected_seeders: seeders,
            half_open_peers: half_open,
            pieces_complete: self
                .inner
                .pieces
                .read()
                .iter()
                .filter(|p| p.complete)
                .count(),
        }
    }

    pub fn seeding(&self) -> bool {
        self.inner.seeding.load(Ordering::Relaxed)
    }

    pub fn download_all(&self) {
        self.inner
            .download_all_requested
            .store(true, Ordering::Relaxed);
        let mut pieces = self.inner.pieces_mut();
        for p in pieces.iter_mut() {
            if p.base_priority == PiecePriority::None {
                p.priority = p.priority.max(PiecePriority::Normal);
                p.base_priority = PiecePriority::Normal;
            }
        }
        drop(pieces);
        self.inner.wake_all_connections();
    }

    pub fn allow_data_download(&self) {
        self.inner
            .data_download_disallowed
            .store(false, Ordering::Relaxed);
        // Download (re)started: find good peers quickly. Armed on creation
        // too, because a new torrent starts allowed and never transitions.
        if self.inner.peer_burst_armed.swap(false, Ordering::Relaxed) {
            self.inner.start_peer_burst();
        }
        self.inner.wake_all_connections();
    }

    /// Network recovery etc.: connect widely again, then trim.
    pub fn start_peer_burst(&self) {
        self.inner.start_peer_burst();
    }

    pub fn disallow_data_download(&self) {
        self.inner
            .data_download_disallowed
            .store(true, Ordering::Relaxed);
        self.inner.peer_burst_armed.store(true, Ordering::Relaxed);
        self.inner.wake_all_connections();
    }

    pub fn allow_data_upload(&self) {
        self.inner
            .data_upload_disallowed
            .store(false, Ordering::Relaxed);
        self.inner.bump_change();
        // Wake peer handlers so on_wake can send Unchoke promptly.
        self.inner.wake_all_connections();
    }

    pub fn disallow_data_upload(&self) {
        self.inner
            .data_upload_disallowed
            .store(true, Ordering::Relaxed);
        self.inner.bump_change();
        // Wake peer handlers so on_wake can send Choke promptly.
        self.inner.wake_all_connections();
    }

    pub fn bump_change(&self) {
        self.inner.bump_change();
    }

    pub fn add_peers(&self, peers: Vec<PeerInfo>) -> usize {
        self.inner.add_peers(peers)
    }

    pub fn add_trackers(&self, tiers: Vec<Vec<String>>) {
        let mut trackers = self.inner.trackers.write();
        let mut seen: std::collections::HashSet<String> =
            trackers.iter().flatten().cloned().collect();
        let added: Vec<Vec<String>> = tiers
            .into_iter()
            .map(|tier| {
                tier.into_iter()
                    .filter(|url| seen.insert(url.clone()))
                    .collect::<Vec<_>>()
            })
            .filter(|tier| !tier.is_empty())
            .collect();
        trackers.extend(added.clone());
        drop(trackers);
        if let Some(client) = self.inner.client.read().upgrade() {
            client.add_trackers(&self.inner, added);
        }
    }

    pub fn add_web_seeds(&self, urls: Vec<String>) {
        self.inner.web_seeds.write().extend(urls);
    }

    /// Go `AnnounceToDht`; here also re-announces to trackers (see [`Self::rediscover`]).
    pub fn announce_to_dht(&self) {
        self.rediscover();
    }

    /// Go network recovery: announce to trackers now and restart DHT lookup.
    pub fn rediscover(&self) {
        if let Some(client) = self.inner.client.read().upgrade() {
            client.rediscover_torrent(&self.inner);
        }
    }

    /// Close every peer connection (Go network recovery closes the old
    /// route's peers instead of waiting for their read deadlines).
    pub fn close_all_peers(&self, reason: &str) {
        let addrs: Vec<SocketAddr> = self.inner.conn_handles.read().keys().copied().collect();
        for addr in addrs {
            self.inner.drop_conn_addr(addr, reason);
        }
    }

    /// Go `SetMaxEstablishedConns`: store the new cap and drop worst
    /// connections until under the limit (worseConn heap order).
    pub fn set_max_established_conns(&self, n: usize) -> usize {
        let old = self.inner.max_established_conns.swap(n, Ordering::Relaxed);
        while self.inner.conn_handles.read().len() > n {
            let Some(victim) = self.inner.worst_conn_addr() else {
                break;
            };
            self.inner
                .drop_conn_addr(victim, "max established conns reduced");
        }
        old
    }

    pub fn swarm_diagnostics(&self) -> stats::SwarmDiagnostics {
        let mut snap = self.inner.swarm_diagnostics.lock().clone();
        snap.pool_size = self.inner.known_peers.read().len();
        snap.dialing = self.inner.dialing_peers.lock().len();
        snap.established = self.inner.conn_handles.read().len();
        snap
    }

    pub fn peer_conns(&self) -> Vec<PeerConnInfo> {
        self.inner.peer_snapshots.read().values().cloned().collect()
    }

    pub fn drop_peer(&self, addr: SocketAddr) -> bool {
        if let Some(handle) = self.inner.conn_handles.read().get(&addr).cloned() {
            handle.close("dropped by caller");
            return true;
        }
        false
    }

    pub fn forget_peers(&self) {
        self.inner.known_peers.write().clear();
    }

    /// Go `Torrent.KnownSwarm`: pending peers plus active connections (a
    /// dialled peer leaves the pending pool, so it must come from its conn).
    pub fn known_swarm(&self) -> Vec<PeerInfo> {
        let mut swarm: Vec<PeerInfo> = self.inner.known_peers.read().iter().cloned().collect();
        for conn in self.inner.peer_snapshots.read().values() {
            let addr = conn.dial_addr.unwrap_or(conn.remote_addr);
            if swarm.iter().any(|p| p.addr == addr) {
                continue;
            }
            swarm.push(PeerInfo {
                id: conn.peer_id,
                addr,
                source: conn.source,
                supports_encryption: conn.encrypted,
                pex: Default::default(),
                trusted: false,
            });
        }
        swarm
    }

    pub fn verify_data(&self, cancel: CancelToken) -> Result<()> {
        let Some(info) = self.inner.info.read().clone() else {
            return Err(CoreError::NoInfo);
        };
        let Some(storage) = self.inner.storage.read().clone() else {
            return Err(CoreError::Other("no storage".to_string()));
        };

        for i in 0..info.num_pieces() {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let piece_obj = info.piece(i);
            let piece_storage = storage.piece(piece_obj, info.piece_hash_v1(i));
            let valid = verify_piece_hash(
                &*piece_storage,
                piece_obj.length as usize,
                info.piece_hash_v1(i),
            );
            let mut pieces = self.inner.pieces_mut();
            if let Some(p) = pieces.get_mut(i as usize) {
                if valid {
                    let _ = piece_storage.mark_complete();
                    p.complete = true;
                    p.ok = true;
                } else {
                    let _ = piece_storage.mark_not_complete();
                    p.complete = false;
                    p.ok = false;
                }
            }
        }
        self.inner.bump_change();
        Ok(())
    }

    pub fn set_on_write_chunk_error(&self, f: Option<Arc<dyn Fn(&CoreError) + Send + Sync>>) {
        *self.inner.on_write_chunk_error.write() = f;
    }

    pub fn new_reader(&self) -> reader::Reader {
        let total_length = self
            .inner
            .info
            .read()
            .as_ref()
            .map(|i| i.total_length())
            .unwrap_or(0);
        reader::Reader::new(self.inner.clone(), 0, total_length)
    }

    pub fn change_version(&self) -> u64 {
        self.inner.change_version.load(Ordering::SeqCst)
    }

    pub fn wait_change(&self, after: u64, timeout: Duration) -> u64 {
        self.inner.wait_version(after, timeout)
    }

    pub fn closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    pub fn drop_torrent(&self) {
        self.inner.closed.store(true, Ordering::Relaxed);
        if let Some(client) = self.inner.client.read().upgrade() {
            client.detach_torrent(&self.inner);
        }
        for handle in self.inner.conn_handles.read().values() {
            handle.close("torrent dropped");
        }
        self.inner.bump_change();
    }

    // ------------------------------------------------ Webtor extensions

    pub fn set_request_focus(&self, torrent_offset: i64) {
        let focus = if torrent_offset < 0 { 0 } else { torrent_offset + 1 };
        let previous = self.inner.request_focus.swap(focus, Ordering::Relaxed);
        // Playback advances this on every read. Waking the whole swarm is only
        // worth it for a jump; peers see small steps on their next refill.
        if previous == 0 || focus == 0 || focus < previous || focus - previous >= FOCUS_WAKE_BYTES {
            self.inner.bump_change();
        }
    }

    /// [`Self::refocus_requests`] for seeks. While a seek bar is dragged only
    /// the first jump and the last one cancel and refill the whole swarm.
    pub fn refocus_requests_debounced(&self) {
        let seq = self.inner.refocus_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut last = self.inner.last_refocus.lock();
        if last.is_none_or(|at| at.elapsed() >= REFOCUS_GAP) {
            *last = Some(std::time::Instant::now());
            drop(last);
            self.refocus_requests();
            return;
        }
        drop(last);
        let torrent = self.clone();
        let handle = self.inner.client.read().upgrade().map(|c| c.tokio_handle.clone());
        let fire = move || {
            if torrent.inner.refocus_seq.load(Ordering::SeqCst) == seq {
                *torrent.inner.last_refocus.lock() = Some(std::time::Instant::now());
                torrent.refocus_requests();
            }
        };
        // Scrubbing calls this constantly; a timer costs no thread per call.
        if let Some(handle) = handle {
            handle.spawn(async move {
                tokio::time::sleep(REFOCUS_GAP).await;
                fire();
            });
        } else {
            std::thread::spawn(move || {
                std::thread::sleep(REFOCUS_GAP);
                fire();
            });
        }
    }

    pub fn refocus_requests(&self) {
        // Opens the seek hedging window for the new playhead.
        *self.inner.seek_hedge.lock() = Some((std::time::Instant::now(), crate::torrent::SEEK_HEDGES));
        self.inner.saved_queue.lock().rewind();
        let handles: Vec<_> = self.inner.conn_handles.read().values().cloned().collect();
        let arrivals: std::collections::HashMap<_, _> = self.inner.live_peers.read().iter()
            .map(|peer| (peer.key, peer.arrival())).collect();
        // Mark the whole cohort before waking anyone. Every handler cancels
        // old non-urgent requests before refilling, with soonest peers woken first.
        let mut ordered = Vec::with_capacity(handles.len());
        for handle in handles {
            handle.mark_refocus();
            let key = crate::torrent_impls::peer_key_from_addr(handle.remote_addr());
            ordered.push((arrivals.get(&key).copied().unwrap_or(f64::MAX), handle));
        }
        ordered.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, handle) in ordered {
            handle.wake();
        }
    }

    pub fn nudge_urgent_requests(&self) -> usize {
        self.inner.nudge_urgent_requests()
    }

    /// Go `NudgeUrgentRequests`; see [`TorrentInner::nudge_urgent_peers`].
    pub fn nudge_urgent_peers(&self) {
        self.inner.nudge_urgent_peers();
    }

    /// Claude seek mode is on (Go `SetUrgentRequestStealing` with a non-zero age).
    pub fn urgent_stealing_enabled(&self) -> bool {
        self.inner.client.read().upgrade()
            .is_some_and(|c| !c.urgent_steal_after.read().is_zero())
    }

    pub fn set_playback_interest(&self, begin: PieceIndex, end: PieceIndex) {
        if begin >= end {
            self.inner.interest_begin.store(0, Ordering::Relaxed);
            self.inner.interest_end.store(0, Ordering::Relaxed);
        } else {
            self.inner
                .interest_begin
                .store(begin as i64 + 1, Ordering::Relaxed);
            self.inner
                .interest_end
                .store(end as i64 + 1, Ordering::Relaxed);
        }
        self.inner.bump_change();
    }

    pub fn peer_counts(&self) -> (usize, usize) {
        let snapshots = self.inner.peer_snapshots.read();
        if !snapshots.is_empty() {
            return (
                snapshots.len(),
                snapshots.values().filter(|s| !s.peer_choking).count(),
            );
        }
        (0, 0)
    }

    /// Go `WebtorDropChokers`: drop peers that keep us choked while interested,
    /// but only when the dial pool is backed up relative to free slots
    /// (`waiting > 2 * free`). Without that gate we killed good seeds (e.g.
    /// 107.175.93.162) while plenty of established slots were still free.
    pub fn drop_chokers(&self, after: Duration) -> usize {
        if self.inner.closed.load(Ordering::Relaxed)
            || self.inner.data_download_disallowed.load(Ordering::Relaxed)
        {
            return 0;
        }
        let waiting = self.inner.known_peers.read().len();
        let established = self.inner.conn_handles.read().len();
        let max_est = self.inner.conn_limit();
        let free = max_est.saturating_sub(established);
        // Go: `if t.peers.Len() <= 2*max(t.maxEstablishedConns-len(t.conns), 0)`
        if waiting <= 2 * free {
            return 0;
        }

        let mut to_drop = Vec::new();
        {
            let snapshots = self.inner.peer_snapshots.read();
            let choked_since = self.inner.choked_since.read();
            for (addr, peer) in snapshots.iter() {
                if to_drop.len() == 2 {
                    break;
                }
                if !peer.peer_choking || !peer.am_interested {
                    continue;
                }
                if choked_since.get(addr).is_some_and(|at| at.elapsed() >= after) {
                    to_drop.push(*addr);
                }
            }
        }
        if to_drop.is_empty() {
            return 0;
        }
        // Go `c.drop()` + deletePeerConn: free the established slot immediately
        // so openNewConns / wants_conns can dial a replacement this tick.
        let mut dropped = 0usize;
        for addr in to_drop {
            if self.inner.conn_handles.read().contains_key(&addr) {
                self.inner.drop_conn_addr(addr, "choked too long");
                // Do not redial it as soon as a tracker reports it again.
                self.inner.dial_backoff.lock().insert(addr, Instant::now() + Duration::from_secs(120));
                dropped += 1;
            }
        }
        if dropped > 0 {
            self.inner
                .chokers_dropped
                .fetch_add(dropped as i64, Ordering::Relaxed);
        }
        dropped
    }

    pub fn chokers_dropped(&self) -> i64 {
        self.inner.chokers_dropped.load(Ordering::Relaxed)
    }

    pub fn rescues(&self) -> i64 {
        self.inner.rescues.load(Ordering::Relaxed)
    }

    pub fn hedges(&self) -> i64 {
        self.inner.hedges_sent.load(Ordering::Relaxed)
    }
}

pub type Reader = reader::Reader;

#[derive(Clone, Default)]
pub struct CancelToken(pub Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------- files

#[derive(Clone)]
pub struct File {
    pub torrent: Arc<TorrentInner>,
    pub file_index: usize,
    pub path: String,
    pub offset: i64,
    pub length: i64,
}

impl File {
    pub fn index(&self) -> usize {
        self.file_index
    }

    pub fn path(&self) -> String {
        self.path.clone()
    }

    pub fn length(&self) -> i64 {
        self.length
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }

    pub fn bytes_completed(&self) -> i64 {
        let Some(info) = self.torrent.info.read().clone() else {
            return 0;
        };
        let file_start = self.offset;
        let file_end = self.offset + self.length;
        let piece_len = info.piece_length;

        let start_piece = (file_start / piece_len) as usize;
        let end_piece = ((file_end + piece_len - 1) / piece_len) as usize;

        let pieces = self.torrent.pieces.read();
        let mut completed = 0;
        for i in start_piece..end_piece.min(pieces.len()) {
            if pieces[i].complete {
                let p_start = i as i64 * piece_len;
                let p_end = (p_start + piece_len).min(info.total_length());
                let overlap_start = file_start.max(p_start);
                let overlap_end = file_end.min(p_end);
                completed += (overlap_end - overlap_start).max(0);
            }
        }
        completed
    }

    pub fn priority(&self) -> PiecePriority {
        let Some(info) = self.torrent.info.read().clone() else {
            return PiecePriority::None;
        };
        let start_piece = (self.offset / info.piece_length) as usize;
        let pieces = self.torrent.pieces.read();
        if start_piece < pieces.len() {
            pieces[start_piece].priority
        } else {
            PiecePriority::None
        }
    }

    pub fn set_priority(&self, p: PiecePriority) {
        let Some(info) = self.torrent.info.read().clone() else {
            return;
        };
        let start_piece = (self.offset / info.piece_length) as usize;
        let end_piece =
            ((self.offset + self.length + info.piece_length - 1) / info.piece_length) as usize;
        let mut pieces = self.torrent.pieces_mut();
        for i in start_piece..end_piece.min(pieces.len()) {
            pieces[i].priority = p;
            pieces[i].base_priority = p;
        }
        drop(pieces);
        self.torrent.bump_change();
    }

    pub fn download(&self) {
        self.set_priority(PiecePriority::Normal);
    }

    pub fn new_reader(&self) -> Reader {
        reader::Reader::new(self.torrent.clone(), self.offset, self.length)
    }
}

pub type ReadaheadFn = Arc<dyn Fn(&ReadaheadContext) -> i64 + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct ReadaheadContext {
    pub current_pos: i64,
    pub contiguous_read_start_pos: i64,
}

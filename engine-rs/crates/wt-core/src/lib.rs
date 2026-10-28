//! BitTorrent client core: pure Rust replacement for the patched
//! `engine-go/patches/torrent` (anacrolix/torrent v1.61.0 + Webtor patches).

#![allow(clippy::type_complexity)]

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use wt_types::*;

pub mod arrival;
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
pub mod reader;
pub mod sources;
pub mod stats;
pub mod storage_adapter;
pub mod strategy;
pub mod torrent;
pub mod torrent_impls;
pub mod tuning;
mod webseed_download;
pub mod webseed_peer;
pub mod webtor;
pub mod wire;

pub use arrival::ArrivalStats;
pub use client::ClientInner;
pub use stats::{ClientStats, ConnStats, PeerConnInfo, TorrentStats};
pub use storage_adapter::{verify_piece_hash, MemoryClientStorage};
pub use torrent::TorrentInner;
pub use tuning::{current_seek_tuning, set_seek_tuning, PeakMode, SeekTuning};
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

impl Client {
    pub fn new(cfg: ClientConfig) -> Result<Client> {
        let inner = ClientInner::new(cfg)?;
        Ok(Client { inner })
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

    pub fn set_outgoing_tcp_conn_hook(&self, hook: Option<TcpConnHook>) {
        self.inner.set_outgoing_tcp_conn_hook(hook);
    }

    pub fn set_urgent_request_stealing(&self, after: Duration) {
        self.inner.set_urgent_request_stealing(after);
    }

    pub fn arrival_stats(&self) -> ArrivalStats {
        let m = self.inner.arrival_model.read();
        ArrivalStats {
            samples: m.samples,
            mean_abs_error_ms: m.net_err * 1000.0,
        }
    }

    pub fn arrival_state(&self) -> Result<Vec<u8>> {
        let m = self.inner.arrival_model.read();
        if m.samples == 0 {
            return Ok(Vec::new());
        }
        serde_json::to_vec(&*m).map_err(|e| CoreError::Other(e.to_string()))
    }

    pub fn set_arrival_state(&self, state: &[u8]) -> Result<()> {
        if state.is_empty() {
            return Ok(());
        }
        let m: arrival::ArrivalModel =
            serde_json::from_slice(state).map_err(|e| CoreError::Other(e.to_string()))?;
        *self.inner.arrival_model.write() = m;
        Ok(())
    }

    pub fn stats(&self) -> ClientStats {
        let mut cs = ConnStats::default();
        let torrents = self.inner.torrents.read();
        for t in torrents.values() {
            let conns = t.conn_handles.read();
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
            let mut guard = self.inner.change_mutex.lock();
            let wait_dur = deadline
                .map(|dl| dl.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(50));
            self.inner
                .change_cond
                .wait_for(&mut guard, wait_dur.min(Duration::from_millis(50)));
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
        let mut pieces = self.inner.pieces.write();
        if let Some(pe) = pieces.get_mut(piece as usize) {
            pe.priority = p;
            pe.base_priority = p;
        }
        drop(pieces);
        self.inner.bump_change();
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
        let Some(storage) = self.inner.storage.read().clone() else {
            return;
        };
        let Some(info) = self.inner.info.read().clone() else {
            return;
        };
        let piece_obj = info.piece(piece);
        let piece_storage = storage.piece(piece_obj, info.piece_hash_v1(piece));
        let comp = piece_storage.completion();
        let complete = comp.ok && comp.complete;
        let evicted = {
            let mut pieces = self.inner.pieces.write();
            if let Some(p) = pieces.get_mut(piece as usize) {
                let evicted = p.complete && !complete;
                p.complete = complete;
                p.ok = comp.ok;
                if evicted {
                    p.partial = false;
                    p.checking = false;
                    p.chunks_received.clear();
                }
                evicted
            } else {
                false
            }
        };
        // Completion and dirty/request bitmaps must agree after bounded storage
        // discards a piece, otherwise the scheduler never requests it again.
        if let Some(book) = self.inner.book.write().as_mut() {
            book.set_complete_cached(piece, comp.ok, complete);
            if evicted {
                let (begin, end) = crate::picker::piece_req_index_range(
                    piece,
                    info.piece_length,
                    info.total_length(),
                );
                let mut dirty = self.inner.dirty_chunks.write();
                for req in begin..end {
                    dirty.remove(&req);
                    book.mark_chunk_pending(req);
                }
            }
        }
        self.inner.bump_change();
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
            let conns = self.inner.conns.read();
            for c in conns.values() {
                cs.add(&c.inner.read().stats.snapshot());
            }
            (
                conns.len(),
                conns
                    .values()
                    .filter(|c| c.inner.read().peer_sent_have_all)
                    .count(),
            )
        };
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
        let mut pieces = self.inner.pieces.write();
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
        self.inner.wake_all_connections();
    }

    pub fn disallow_data_download(&self) {
        self.inner
            .data_download_disallowed
            .store(true, Ordering::Relaxed);
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

    pub fn announce_to_dht(&self) {}

    pub fn set_max_established_conns(&self, n: usize) -> usize {
        self.inner.max_established_conns.swap(n, Ordering::Relaxed)
    }

    pub fn swarm_diagnostics(&self) -> stats::SwarmDiagnostics {
        let mut snap = self.inner.swarm_diagnostics.lock().clone();
        snap.pool_size = self.inner.known_peers.read().len();
        snap.dialing = self.inner.dialing_peers.lock().len();
        snap.established = self.inner.conn_handles.read().len();
        snap
    }

    pub fn peer_conns(&self) -> Vec<PeerConnInfo> {
        let snapshots = self.inner.peer_snapshots.read();
        if !snapshots.is_empty() {
            return snapshots.values().cloned().collect();
        }
        drop(snapshots);
        let conns = self.inner.conns.read();
        conns
            .values()
            .map(|c| {
                let inner = c.inner.read();
                PeerConnInfo {
                    remote_addr: inner.remote_addr,
                    dial_addr: c.cache_dial_addr(),
                    peer_id: inner.peer_id,
                    client_name: inner.client_name.clone(),
                    transport: inner.transport,
                    source: inner.source,
                    encrypted: inner.encrypted,
                    outgoing: inner.outgoing,
                    peer_choking: inner.peer_choking,
                    am_choking: inner.am_choking,
                    peer_interested: inner.peer_interested,
                    am_interested: inner.am_interested,
                    conn_stats: inner.stats.snapshot(),
                    download_rate: inner.download_rate.rate(),
                    upload_rate: inner.upload_rate.rate(),
                    peer_completion: if inner.peer_sent_have_all {
                        1.0
                    } else if self.num_pieces() > 0 {
                        inner.peer_pieces.len() as f64 / self.num_pieces() as f64
                    } else {
                        0.0
                    },
                    pending_requests: inner.outstanding_requests.len(),
                    peer_max_requests: inner.max_requests,
                    peak_requests: inner.peak_requests,
                    pipeline_limit: inner.webtor_pipeline_limit,
                    nominal_max_requests: inner.nominal_max_requests(),
                    rtt: inner.rtt.estimate,
                    queued: inner.outstanding_requests.len(),
                    cancelled: inner.cancelled_requests.len(),
                    connected_for: inner.connected_at.elapsed(),
                    last_useful_data: inner.last_useful_chunk_received.map(|t| t.elapsed()),
                }
            })
            .collect()
    }

    pub fn drop_peer(&self, addr: SocketAddr) -> bool {
        if let Some(handle) = self.inner.conn_handles.read().get(&addr).cloned() {
            handle.close("dropped by caller");
            return true;
        }
        let mut conns = self.inner.conns.write();
        if let Some(conn) = conns.remove(&addr) {
            conn.close();
            drop(conns);
            self.inner.bump_change();
            true
        } else {
            false
        }
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
            let mut pieces = self.inner.pieces.write();
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
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.change_mutex.lock();
        loop {
            let cur = self.inner.change_version.load(Ordering::SeqCst);
            if cur > after || Instant::now() >= deadline {
                return cur;
            }
            let wait_dur = deadline.saturating_duration_since(Instant::now());
            self.inner.change_cond.wait_for(&mut guard, wait_dur);
        }
    }

    pub fn closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    pub fn drop_torrent(&self) {
        self.inner.closed.store(true, Ordering::Relaxed);
        for handle in self.inner.conn_handles.read().values() {
            handle.close("torrent dropped");
        }
        let conns = self.inner.conns.write();
        for c in conns.values() {
            c.close();
        }
        drop(conns);
        self.inner.bump_change();
    }

    // ------------------------------------------------ Webtor extensions

    pub fn set_request_focus(&self, torrent_offset: i64) {
        if torrent_offset < 0 {
            self.inner.request_focus.store(0, Ordering::Relaxed);
        } else {
            self.inner
                .request_focus
                .store(torrent_offset + 1, Ordering::Relaxed);
        }
        self.inner.bump_change();
    }

    pub fn refocus_requests(&self) {
        for handle in self.inner.conn_handles.read().values() {
            handle.request_refocus();
        }
        self.inner.wake_all_connections();
        let conns = self.inner.conns.read();
        for c in conns.values() {
            self.inner.refill_peer_requests(c);
        }
    }

    pub fn nudge_urgent_requests(&self) -> usize {
        self.inner.nudge_urgent_requests()
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
        drop(snapshots);
        let conns = self.inner.conns.read();
        let total = conns.len();
        let unchoked = conns
            .values()
            .filter(|c| !c.inner.read().peer_choking)
            .count();
        (total, unchoked)
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
        let max_est = self.inner.max_established_conns.load(Ordering::Relaxed);
        let free = max_est.saturating_sub(established);
        // Go: `if t.peers.Len() <= 2*max(t.maxEstablishedConns-len(t.conns), 0)`
        if waiting <= 2 * free {
            return 0;
        }

        let mut to_drop = Vec::new();
        {
            let conns = self.inner.conns.read();
            for (&addr, c) in conns.iter() {
                if to_drop.len() == 2 {
                    break;
                }
                let inner = c.inner.read();
                if !inner.peer_choking || !inner.am_interested {
                    continue;
                }
                // Go: since = max(webtorChokedSince, lastBecameInterested)
                let mut since = inner.webtor_choked_since;
                if let Some(li) = inner.last_became_interested {
                    since = Some(match since {
                        Some(s) if li > s => li,
                        Some(s) => s,
                        None => li,
                    });
                }
                if since.is_some_and(|s| s.elapsed() >= after) {
                    to_drop.push(addr);
                }
            }
        }
        if to_drop.is_empty() {
            return 0;
        }
        let handles = self.inner.conn_handles.read();
        let mut dropped = 0usize;
        for addr in &to_drop {
            if let Some(handle) = handles.get(addr) {
                handle.close("choked too long");
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
        let mut pieces = self.torrent.pieces.write();
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

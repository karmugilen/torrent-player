//! BitTorrent client core: the Rust replacement for the patched
//! `engine-go/patches/torrent` (anacrolix/torrent v1.61.0 + Webtor patches).
//!
//! CONTRACT: the public items below are what `wt-engine` is written against.
//! The `wt-core` owner implements them and may add items freely, but must ask
//! the coordinator before renaming, removing or changing a signature here.
//!
//! The API is blocking, like the Go one: methods take an internal lock and
//! return; `Reader::read` blocks until data is verified. Networking runs on an
//! internal Tokio runtime owned by `Client`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

pub use wt_types::*;

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
    pub extended_handshake_client_version: String,
    pub ice_servers: Vec<String>,
    pub default_storage: Arc<dyn ClientStorage>,
    pub callbacks: Callbacks,
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
    TrackerConnected { url: String, info_hash: InfoHash },
    TrackerDisconnected { url: String, info_hash: InfoHash, error: Option<String> },
    TrackerAnnounceSuccessful { url: String, info_hash: InfoHash },
    TrackerAnnounceError { url: String, info_hash: InfoHash, error: String },
}

/// Webtor seek rules (`torrent.SeekTuning` in webtor_urgent.go). Field
/// meanings are documented on the Go struct.
#[derive(Clone, Debug, PartialEq)]
pub struct SeekTuning {
    pub focus_blocks: i32,
    pub focus_steal_divisor: i32,
    pub faster_ratio: f64,
    pub queue_aware: bool,
    pub bound_candidates: bool,
    pub pipeline_seconds: f64,
    pub min_pipeline: i32,
    pub full_pipeline: bool,
    pub pipeline_rtt: f64,
    pub arrival_aware: bool,
    pub arrival_ratio: f64,
    pub no_balance_steal: bool,
    pub refill_at: f64,
    pub hol_rescue: bool,
    pub hol_after_ms: f64,
    pub hedge: bool,
    pub slow_peer_yield: bool,
    pub learned_arrival: bool,
    pub peak_mode: PeakMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PeakMode {
    /// Upstream: the peak is the blocks the update added.
    #[default]
    Added,
    /// The peak is every outstanding request.
    WholeQueue,
    /// Keep the peak when a busy peer is topped up; only a refill from empty sets it.
    KeepOnTopUp,
}

impl Default for SeekTuning {
    /// `DefaultSeekTuning` in Go.
    fn default() -> Self {
        SeekTuning {
            focus_blocks: 8,
            focus_steal_divisor: 2,
            faster_ratio: 3.0,
            queue_aware: false,
            bound_candidates: true,
            pipeline_seconds: 0.0,
            min_pipeline: 0,
            full_pipeline: false,
            pipeline_rtt: 2.0,
            arrival_aware: true,
            arrival_ratio: 1.5,
            no_balance_steal: false,
            refill_at: 0.0,
            hol_rescue: false,
            hol_after_ms: 300.0,
            hedge: false,
            slow_peer_yield: false,
            learned_arrival: false,
            peak_mode: PeakMode::WholeQueue,
        }
    }
}

/// Process-wide seek rules for every client (`torrent.SetSeekTuning`;
/// clamps `focus_blocks` and `focus_steal_divisor` to at least 1).
pub fn set_seek_tuning(_tuning: SeekTuning) {
    unimplemented!()
}
pub fn current_seek_tuning() -> SeekTuning {
    unimplemented!()
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
    _inner: Arc<()>,
}

/// Wraps outgoing TCP peer streams (`Client.SetOutgoingTCPConnHook`, used by anti-DPI).
pub type TcpConnHook = Arc<dyn Fn(tokio::net::TcpStream) -> Box<dyn PeerStream> + Send + Sync>;

/// Any byte stream a peer connection can run on.
pub trait PeerStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static> PeerStream for T {}

impl Client {
    pub fn new(_cfg: ClientConfig) -> Result<Client> {
        unimplemented!()
    }
    /// Returns the torrent and whether it was newly added.
    pub fn add_torrent_spec(&self, _spec: TorrentSpec) -> Result<(Torrent, bool)> {
        unimplemented!()
    }
    pub fn torrent(&self, _ih: InfoHash) -> Option<Torrent> {
        unimplemented!()
    }
    pub fn torrents(&self) -> Vec<Torrent> {
        unimplemented!()
    }
    pub fn local_port(&self) -> u16 {
        unimplemented!()
    }
    pub fn listen_addrs(&self) -> Vec<SocketAddr> {
        unimplemented!()
    }
    pub fn add_dht_nodes(&self, _nodes: &[String]) {
        unimplemented!()
    }
    /// Good DHT contacts, for persisting across restarts.
    pub fn dht_contacts(&self) -> Vec<SocketAddr> {
        unimplemented!()
    }
    pub fn set_outgoing_tcp_conn_hook(&self, _hook: Option<TcpConnHook>) {
        unimplemented!()
    }
    /// Zero disables (upstream behaviour).
    pub fn set_urgent_request_stealing(&self, _after: Duration) {
        unimplemented!()
    }
    pub fn arrival_stats(&self) -> ArrivalStats {
        unimplemented!()
    }
    /// Serialized learned block-arrival model (webtor_arrival.go).
    pub fn arrival_state(&self) -> Result<Vec<u8>> {
        unimplemented!()
    }
    pub fn set_arrival_state(&self, _state: &[u8]) -> Result<()> {
        unimplemented!()
    }
    pub fn stats(&self) -> ClientStats {
        unimplemented!()
    }
    pub fn close(&self) {
        unimplemented!()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArrivalStats {
    pub samples: u64,
    pub mean_abs_error_ms: f64,
}

#[derive(Clone, Debug, Default)]
pub struct ClientStats {
    pub conn_stats: ConnStats,
    pub active_half_open: usize,
}

/// Byte/chunk counters (`torrent.ConnStats`).
#[derive(Clone, Debug, Default)]
pub struct ConnStats {
    pub bytes_written: i64,
    pub bytes_written_data: i64,
    pub bytes_read: i64,
    pub bytes_read_data: i64,
    pub bytes_read_useful_data: i64,
    pub bytes_read_useful_intended_data: i64,
    pub chunks_written: i64,
    pub chunks_read: i64,
    pub chunks_read_useful: i64,
    pub chunks_read_wasted: i64,
    pub pieces_dirtied_good: i64,
    pub pieces_dirtied_bad: i64,
}

// ---------------------------------------------------------------- torrent

#[derive(Clone)]
pub struct Torrent {
    _inner: Arc<()>,
}

#[derive(Clone, Debug, Default)]
pub struct TorrentStats {
    pub conn_stats: ConnStats,
    pub total_peers: usize,
    pub pending_peers: usize,
    pub active_peers: usize,
    pub connected_seeders: usize,
    pub half_open_peers: usize,
    pub pieces_complete: usize,
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
        unimplemented!()
    }
    pub fn info_hash_v2(&self) -> Option<InfoHashV2> {
        unimplemented!()
    }
    pub fn name(&self) -> String {
        unimplemented!()
    }
    /// `None` until metadata arrives.
    pub fn info(&self) -> Option<Arc<Info>> {
        unimplemented!()
    }
    /// Blocks until metadata arrives, the timeout passes, or the torrent closes.
    pub fn wait_info(&self, _timeout: Option<Duration>) -> Result<Arc<Info>> {
        unimplemented!()
    }
    pub fn metainfo(&self) -> Result<MetaInfo> {
        unimplemented!()
    }
    pub fn files(&self) -> Vec<File> {
        unimplemented!()
    }
    pub fn num_pieces(&self) -> u32 {
        unimplemented!()
    }
    pub fn set_piece_priority(&self, _piece: PieceIndex, _p: PiecePriority) {
        unimplemented!()
    }
    pub fn piece_state(&self, _piece: PieceIndex) -> PieceState {
        unimplemented!()
    }
    pub fn piece_state_runs(&self) -> Vec<PieceStateRun> {
        unimplemented!()
    }
    pub fn piece_complete(&self, _piece: PieceIndex) -> bool {
        unimplemented!()
    }
    /// Re-read storage completion (`Piece.UpdateCompletion`).
    pub fn update_piece_completion(&self, _piece: PieceIndex) {
        unimplemented!()
    }
    pub fn bytes_completed(&self) -> i64 {
        unimplemented!()
    }
    pub fn bytes_missing(&self) -> i64 {
        unimplemented!()
    }
    pub fn stats(&self) -> TorrentStats {
        unimplemented!()
    }
    pub fn seeding(&self) -> bool {
        unimplemented!()
    }
    pub fn download_all(&self) {
        unimplemented!()
    }
    pub fn allow_data_download(&self) {
        unimplemented!()
    }
    pub fn disallow_data_download(&self) {
        unimplemented!()
    }
    pub fn allow_data_upload(&self) {
        unimplemented!()
    }
    pub fn disallow_data_upload(&self) {
        unimplemented!()
    }
    pub fn add_peers(&self, _peers: Vec<PeerInfo>) -> usize {
        unimplemented!()
    }
    pub fn add_trackers(&self, _tiers: Vec<Vec<String>>) {
        unimplemented!()
    }
    pub fn add_web_seeds(&self, _urls: Vec<String>) {
        unimplemented!()
    }
    pub fn announce_to_dht(&self) {
        unimplemented!()
    }
    pub fn set_max_established_conns(&self, _n: usize) -> usize {
        unimplemented!()
    }
    pub fn peer_conns(&self) -> Vec<PeerConnInfo> {
        unimplemented!()
    }
    /// Disconnect one peer by remote address.
    pub fn drop_peer(&self, _addr: SocketAddr) -> bool {
        unimplemented!()
    }
    /// Forget every known-but-unconnected peer.
    pub fn forget_peers(&self) {
        unimplemented!()
    }
    pub fn known_swarm(&self) -> Vec<PeerInfo> {
        unimplemented!()
    }
    /// Hash every piece against storage (`VerifyDataContext`); progress via `piece_state`.
    pub fn verify_data(&self, _cancel: CancelToken) -> Result<()> {
        unimplemented!()
    }
    /// Callback for storage write failures (`SetOnWriteChunkError`).
    pub fn set_on_write_chunk_error(&self, _f: Option<Arc<dyn Fn(&CoreError) + Send + Sync>>) {
        unimplemented!()
    }
    pub fn new_reader(&self) -> Reader {
        unimplemented!()
    }
    /// Increments on any piece state, peer or stats change; pair with `wait_change`.
    pub fn change_version(&self) -> u64 {
        unimplemented!()
    }
    pub fn wait_change(&self, _after: u64, _timeout: Duration) -> u64 {
        unimplemented!()
    }
    pub fn closed(&self) -> bool {
        unimplemented!()
    }
    pub fn drop_torrent(&self) {
        unimplemented!()
    }

    // ------------------------------------------------ Webtor extensions
    // (webtor_urgent.go). Same semantics as the Go patch.

    pub fn set_request_focus(&self, _torrent_offset: i64) {
        unimplemented!()
    }
    pub fn refocus_requests(&self) {
        unimplemented!()
    }
    pub fn nudge_urgent_requests(&self) -> usize {
        unimplemented!()
    }
    /// Pieces [begin, end) are the playing window.
    pub fn set_playback_interest(&self, _begin: PieceIndex, _end: PieceIndex) {
        unimplemented!()
    }
    pub fn peer_counts(&self) -> (usize, usize) {
        unimplemented!()
    }
    pub fn drop_chokers(&self, _after: Duration) -> usize {
        unimplemented!()
    }
    pub fn chokers_dropped(&self) -> i64 {
        unimplemented!()
    }
    pub fn rescues(&self) -> i64 {
        unimplemented!()
    }
    pub fn hedges(&self) -> i64 {
        unimplemented!()
    }
}

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
    _inner: Arc<()>,
}

impl File {
    pub fn index(&self) -> usize {
        unimplemented!()
    }
    /// Display path: torrent name joined with the file path, '/'-separated.
    pub fn path(&self) -> String {
        unimplemented!()
    }
    pub fn length(&self) -> i64 {
        unimplemented!()
    }
    pub fn offset(&self) -> i64 {
        unimplemented!()
    }
    pub fn bytes_completed(&self) -> i64 {
        unimplemented!()
    }
    pub fn set_priority(&self, _p: PiecePriority) {
        unimplemented!()
    }
    pub fn download(&self) {
        unimplemented!()
    }
    /// Reader limited to this file; offsets are file-relative.
    pub fn new_reader(&self) -> Reader {
        unimplemented!()
    }
}

// ---------------------------------------------------------------- readers

/// Readahead as a function of the reader state (`SetReadaheadFunc`).
pub type ReadaheadFn = Arc<dyn Fn(&ReadaheadContext) -> i64 + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct ReadaheadContext {
    pub current_pos: i64,
    pub contiguous_read_start_pos: i64,
}

/// Blocking, seekable reader (`torrent.Reader`). Reading marks the pieces under
/// the position `Now` and the readahead window `Readahead`/`Next`, and blocks
/// until the bytes are verified.
pub struct Reader {
    _inner: (),
}

impl Reader {
    pub fn set_readahead(&mut self, _bytes: i64) {
        unimplemented!()
    }
    pub fn set_readahead_fn(&mut self, _f: Option<ReadaheadFn>) {
        unimplemented!()
    }
    /// Return as soon as any bytes are available, not a full buffer.
    pub fn set_responsive(&mut self) {
        unimplemented!()
    }
    /// Webtor patch: `WebtorSetResponsive(on)`.
    pub fn set_webtor_responsive(&mut self, _on: bool) {
        unimplemented!()
    }
    /// Reads fail with `Interrupted` once cancelled (`SetContext`).
    pub fn set_cancel(&mut self, _cancel: CancelToken) {
        unimplemented!()
    }
    pub fn read_timeout(&mut self, _buf: &mut [u8], _timeout: Option<Duration>) -> std::io::Result<usize> {
        unimplemented!()
    }
}

impl std::io::Read for Reader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        unimplemented!()
    }
}

impl std::io::Seek for Reader {
    fn seek(&mut self, _pos: std::io::SeekFrom) -> std::io::Result<u64> {
        unimplemented!()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {}
}

// ---------------------------------------------------------------- peers

/// Snapshot of one live connection, for the details screen and peer policies.
#[derive(Clone, Debug)]
pub struct PeerConnInfo {
    pub remote_addr: SocketAddr,
    /// Dialable endpoint for the peer cache (`CacheDialAddr`), if any.
    pub dial_addr: Option<SocketAddr>,
    pub peer_id: Option<PeerId>,
    pub client_name: String,
    pub transport: Transport,
    pub source: PeerSource,
    pub encrypted: bool,
    pub outgoing: bool,
    pub peer_choking: bool,
    pub am_choking: bool,
    pub peer_interested: bool,
    pub am_interested: bool,
    pub conn_stats: ConnStats,
    pub download_rate: f64,
    pub upload_rate: f64,
    /// Fraction of the torrent the peer has, 0..1.
    pub peer_completion: f64,
    pub pending_requests: usize,
    pub rtt: Duration,
    pub queued: usize,
    pub cancelled: usize,
    pub connected_for: Duration,
    pub last_useful_data: Option<Duration>,
}

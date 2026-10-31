//! Peer connection state machine (`peerconn.go` / `peer.go` / `requesting.go`).
//!
//! # Integration (agy-2)
//!
//! Implement [`PeerTorrent`] on `TorrentInner`. Construct a [`PeerConnHandler`]
//! and pass it to `conn::spawn_connection_driver` with a [`ConnHandle`].
//!
//! Legacy [`PeerConn`] / [`PeerConnInner`] remain for the current client/torrent
//! paths until they migrate onto [`PeerConnHandler`].
//!
//! ## `PeerTorrent` (agy-2 implements)
//!
//! ```ignore
//! pub type PeerKey = u64;
//!
//! pub trait PeerTorrent: Send + Sync {
//!     fn peer_key(&self, addr: SocketAddr) -> PeerKey;
//!     fn have_info(&self) -> bool;
//!     fn num_pieces(&self) -> u32;
//!     fn piece_length(&self) -> i64;
//!     fn total_length(&self) -> i64;
//!     fn chunk_size(&self) -> u32;
//!     fn info_hash_bytes(&self) -> [u8; 20];
//!     fn data_download_disallowed(&self) -> bool;
//!     fn data_upload_disallowed(&self) -> bool;
//!     fn is_seeding(&self) -> bool;
//!     fn closed(&self) -> bool;
//!
//!     fn piece_complete(&self, piece: PieceIndex) -> bool;
//!     fn piece_priority(&self, piece: PieceIndex) -> PiecePriority;
//!     fn chunk_dirty(&self, req: RequestIndex) -> bool;
//!     fn mark_chunk_dirty(&self, req: RequestIndex);
//!     fn clear_chunk_requested(&self, req: RequestIndex);
//!     fn mark_chunk_requested(&self, req: RequestIndex);
//!     fn piece_all_dirty(&self, piece: PieceIndex) -> bool;
//!     fn note_availability(&self, piece: PieceIndex, delta: i32);
//!
//!     fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()>;
//!     fn report_write_chunk_error(&self, err: &CoreError);
//!     fn read_chunk(&self, piece: PieceIndex, begin: u32, length: u32) -> Result<Bytes>;
//!     fn queue_piece_hash(&self, piece: PieceIndex);
//!
//!     fn requesting_peer(&self, req: RequestIndex) -> Option<PeerKey>;
//!     fn request_when(&self, req: RequestIndex) -> Option<Instant>;
//!     fn assign_request(&self, req: RequestIndex, peer: PeerKey, when: Instant);
//!     fn clear_assignment(&self, req: RequestIndex);
//!     fn desired_request_context(&self) -> (RequestTorrentSnapshot, PieceRequestOrder);
//!
//!     fn request_focus_index(&self) -> Option<RequestIndex>;
//!     fn urgent_steal_after(&self) -> Duration;
//!     fn seek_tuning(&self) -> SeekTuning;
//!     fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey>;
//!     fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey);
//!     fn clear_hedge(&self, req: RequestIndex);
//!     fn record_rescue(&self);
//!     fn record_hedge(&self);
//!     fn peer_has_piece(&self, peer: PeerKey, piece: PieceIndex) -> bool;
//!     fn webtor_peer_snap(&self, peer: PeerKey) -> Option<WebtorPeerSnap>;
//!     fn connected_peer_keys(&self) -> Vec<PeerKey>;
//!
//!     fn on_metadata_data(&self, piece: u32, total_size: u32, data: Bytes) -> Result<bool>;
//!     fn metadata_piece(&self, piece: u32) -> Option<(u32, Bytes)>; // (total_size, data)
//!     fn metadata_size(&self) -> Option<u32>;
//!     fn has_metadata(&self) -> bool;
//!     fn ingest_pex(&self, from: SocketAddr, msg: &PexMsg) -> Result<()>;
//!     fn add_dht_node(&self, addr: SocketAddr);
//!     fn on_peer_listen_port(&self, peer_addr: SocketAddr, port: u16);
//!
//!     fn choke_config(&self) -> ChokeConfig;
//!     fn bump_change(&self);
//!     fn on_chunk_received(&self, peer: PeerKey, req: RequestIndex, len: usize);
//! }
//! ```

#![allow(clippy::too_many_arguments)]

use bytes::Bytes;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify};
use wt_types::{
    PeerId, PeerSource, PieceIndex, PiecePriority, RequestIndex, Transport, BLOCK_SIZE,
};
use wt_wire::ext::{
    ExtendedHandshake, MetadataMsg, MetadataMsgType, PexMsg, EXTENSION_NAME_METADATA,
    EXTENSION_NAME_PEX,
};
use wt_wire::handshake::PeerExtensionBits;
use wt_wire::message::{unmarshal_bitfield, Message};

/// Go `PeerMaxRequests` default before the peer's LTEP `reqq` arrives.
pub const DEFAULT_PEER_MAX_REQUESTS: usize = 250;
/// Go `localClientReqq` — how many requests we advertise we can accept.
pub const LOCAL_CLIENT_REQQ: u32 = 1024;

use crate::arrival::{ArrivalNet, ArrivalPeer, ArrivalSample, PeerArrivalState, ARRIVAL_FEATURES};
use crate::choking::{upload_allowed, ChokeConfig, PeerChokeInput};
use crate::conn::{ConnHandle, ConnHandler};
use crate::metadata::{PeerMetadataState, METADATA_REQUEST_TIMEOUT};
use crate::pex::{PexCursor, PEX_INTERVAL, PEX_RETRY_DELAY};
use crate::picker::{
    block_coord_from_req_index, piece_req_index_range,
    req_index_from_piece_and_offset,
};
use crate::stats::{AtomicConnStats, ExpectingRate, PeerConnInfo, RateEstimator};

/// Go `maxLocalToRemoteRequests`: (writeBufferHighWaterLen -
/// writeBufferLowWaterLen - interestedMsgLen) / requestMsgLen with 1 MiB / 512 KiB
/// buffers, 5-byte interested and 17-byte request messages.
pub(crate) const MAX_LOCAL_TO_REMOTE_REQUESTS: usize = ((1 << 20) - (1 << 19) - 5) / 17;
use crate::strategy::{
    compute_desired_requests, sort_requests_for_peer, DesiredPeerSnapshot, PieceRequestOrder,
    RequestTorrentSnapshot, RequestTuning,
};
use crate::tuning::{current_seek_tuning, SeekTuning};
use crate::webtor::{
    expected_arrival_secs, hedge_decisions, may_take_existing_request,
    next_peak_requests, pending_requests, rescue_decisions, should_steal_urgent,
    steal_lands_in_flight, PeerView, SystemClock, TorrentView,
};
use crate::wire::{DEFAULT_UT_METADATA_ID, DEFAULT_UT_PEX_ID, EXT_HANDSHAKE_ID};
use crate::{CoreError, Result};

// ---------------------------------------------------------------- PeerTorrent

/// Opaque peer identity in torrent assignment maps.
pub type PeerKey = u64;

/// What the saved-download picker needs to know about a peer.
pub struct BackgroundPeer<'a> {
    pub key: PeerKey,
    pub have_all: bool,
    pub pieces: &'a HashSet<PieceIndex>,
    pub allowed_fast: &'a HashSet<PieceIndex>,
    pub choking: bool,
    pub cancelled: &'a HashSet<RequestIndex>,
    /// About to run dry: may start a piece even when RAM is spoken for.
    pub starving: bool,
}

/// Blocks a peer could fetch next for a saved download.
#[derive(Default)]
pub struct BackgroundPick {
    /// Nobody holds these.
    pub free: Vec<RequestIndex>,
    /// Other peers hold these. Only filled at the very end.
    pub held: Vec<RequestIndex>,
    /// The peer has something we want.
    pub interested: bool,
    pub wanted_bytes: i64,
}

/// Snapshot of another peer for Webtor faster-focus / HOL decisions.
#[derive(Clone, Debug)]
pub struct WebtorPeerSnap {
    pub key: PeerKey,
    pub download_rate_bps: f64,
    pub rtt: Duration,
    pub queued: u32,
    pub cancelled: u32,
    pub nominal_max: u32,
    pub peer_choking: bool,
    pub closed: bool,
    pub last_useful: Option<Instant>,
}

/// Torrent-wide surface the peer state machine needs. Implemented by agy-2 on
/// `TorrentInner` (or a test double).
pub trait PeerTorrent: Send + Sync + 'static {
    fn peer_key(&self, addr: SocketAddr) -> PeerKey;
    fn have_info(&self) -> bool;
    fn num_pieces(&self) -> u32;
    fn piece_length(&self) -> i64;
    fn total_length(&self) -> i64;
    fn chunk_size(&self) -> u32 {
        BLOCK_SIZE
    }
    fn info_hash_bytes(&self) -> [u8; 20];
    fn data_download_disallowed(&self) -> bool;
    fn data_upload_disallowed(&self) -> bool;
    fn is_seeding(&self) -> bool;
    fn closed(&self) -> bool;

    fn piece_complete(&self, piece: PieceIndex) -> bool;
    fn piece_hashing(&self, _piece: PieceIndex) -> bool { false }
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority;
    fn chunk_dirty(&self, req: RequestIndex) -> bool;
    fn mark_chunk_dirty(&self, req: RequestIndex);
    fn clear_chunk_requested(&self, req: RequestIndex);
    fn mark_chunk_requested(&self, req: RequestIndex);
    fn piece_all_dirty(&self, piece: PieceIndex) -> bool;
    fn note_availability(&self, piece: PieceIndex, delta: i32);
    /// This peer just claimed a block at the playhead. Peers that refilled
    /// before the claim had nothing to hedge; let them look again.
    fn focus_block_claimed(&self) {}
    /// Playback is on and this torrent may send a warm-up request now.
    fn may_warm(&self) -> bool {
        false
    }
    /// A have-all peer joined (`1`) or left (`-1`).
    fn note_have_all_availability(&self, delta: i32) {
        for piece in 0..self.num_pieces() {
            self.note_availability(piece, delta);
        }
    }
    fn note_peer_piece(&self, _peer: PeerKey, _piece: PieceIndex, _has: bool) {}
    /// [`Self::note_peer_piece`] for many pieces at once.
    fn note_peer_pieces(&self, peer: PeerKey, pieces: &mut dyn Iterator<Item = PieceIndex>) {
        for piece in pieces {
            self.note_peer_piece(peer, piece, true);
        }
    }
    /// Those of `requests` that neither `peer` nor its hedge holds any more.
    fn displaced_requests(&self, requests: &HashSet<RequestIndex>, peer: PeerKey) -> Vec<RequestIndex> {
        requests
            .iter()
            .copied()
            .filter(|&req| self.requesting_peer(req) != Some(peer) && self.hedge_peer(req) != Some(peer))
            .collect()
    }
    fn peer_closed(&self, _addr: SocketAddr) {}
    fn update_peer_snapshot(&self, _info: PeerConnInfo) {}
    /// Peer delivered useful payload — clear dial fail/backoff for this addr.
    fn note_peer_helpful(&self, _addr: SocketAddr) {}

    fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()>;
    /// Nothing near a playhead is wanted: refills use the saved-download queue.
    fn background_only(&self) -> bool {
        false
    }
    fn background_pick(&self, _peer: &BackgroundPeer<'_>, _want: usize) -> BackgroundPick {
        BackgroundPick::default()
    }
    /// `read_chunk` when the block is in RAM; `None` when it needs the disk.
    fn read_chunk_in_memory(&self, _piece: PieceIndex, _begin: u32, _length: u32) -> Option<Result<Bytes>> {
        None
    }
    /// `write_chunk` when it only copies into RAM; `None`, having written
    /// nothing, when it would wait on disk (`PieceStorage::write_at_in_memory`).
    fn write_chunk_in_memory(&self, _piece: PieceIndex, _begin: u32, _data: &[u8]) -> Option<Result<()>> {
        None
    }
    /// Go `haveChunk` + `incrementPendingWrites`: atomically reserve a received
    /// block for writing. False when it is already written, another peer is
    /// writing it, or its piece is complete.
    fn begin_chunk_write(&self, req: RequestIndex) -> bool {
        !self.chunk_dirty(req)
    }
    /// End a [`Self::begin_chunk_write`] reservation. Only a successful write
    /// marks the block dirty, so readers never see unwritten data.
    fn finish_chunk_write(&self, req: RequestIndex, written: bool) {
        if written {
            self.mark_chunk_dirty(req);
        }
    }
    /// Go `peerHasWantedPieces` torrent half: any incomplete piece is wanted.
    fn any_wanted_incomplete(&self) -> bool {
        (0..self.num_pieces()).any(|i| {
            self.piece_priority(i) > PiecePriority::None && !self.piece_complete(i)
        })
    }
    /// Whether any of `pieces` is wanted and incomplete.
    fn any_wanted_among(&self, pieces: &HashSet<PieceIndex>) -> bool {
        pieces.iter().any(|&p| self.piece_priority(p) > PiecePriority::None && !self.piece_complete(p))
    }
    /// Go `Torrent.onWriteChunkErr` — storage/cache write failures while receiving.
    fn report_write_chunk_error(&self, _err: &CoreError) {}
    fn read_chunk(&self, piece: PieceIndex, begin: u32, length: u32) -> Result<Bytes>;
    fn queue_piece_hash(&self, piece: PieceIndex);

    fn requesting_peer(&self, req: RequestIndex) -> Option<PeerKey>;
    fn request_when(&self, req: RequestIndex) -> Option<Instant>;
    fn assign_request(&self, req: RequestIndex, peer: PeerKey, when: Instant);
    fn claim_request(&self, req: RequestIndex, peer: PeerKey, when: Instant) -> bool {
        if self.requesting_peer(req).is_some_and(|owner| owner != peer) { return false; }
        self.assign_request(req, peer, when);
        true
    }
    fn release_request(&self, req: RequestIndex, peer: PeerKey) {
        if self.requesting_peer(req) == Some(peer) { self.clear_assignment(req); }
    }
    fn clear_assignment(&self, req: RequestIndex);
    fn desired_request_context(&self) -> (RequestTorrentSnapshot, PieceRequestOrder);
    fn desired_request_context_for_peer(&self, _peer: &DesiredPeerSnapshot) -> (RequestTorrentSnapshot, PieceRequestOrder) {
        self.desired_request_context()
    }

    fn request_focus_index(&self) -> Option<RequestIndex>;
    /// A seek moved the playhead moments ago and eager hedges are left.
    fn seek_hedge_open(&self) -> bool {
        false
    }
    /// Spend one of the seek's eager hedges.
    fn take_seek_hedge(&self) -> bool {
        false
    }
    /// How many of `peer`'s outstanding requests were sent before `when`.
    /// When each of this peer's current requests was assigned, oldest first.
    fn request_times(&self, _peer: PeerKey) -> Vec<Instant> {
        Vec::new()
    }
    fn refund_seek_hedge(&self) {}
    fn urgent_steal_after(&self) -> Duration;
    fn seek_tuning(&self) -> SeekTuning {
        current_seek_tuning()
    }
    fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey>;
    fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey);
    fn claim_hedge(&self, req: RequestIndex, peer: PeerKey) -> bool {
        if self.hedge_peer(req).is_some() { return false; }
        self.set_hedge_peer(req, peer);
        true
    }
    fn clear_hedge(&self, req: RequestIndex);
    fn release_hedge(&self, req: RequestIndex, peer: PeerKey) {
        if self.hedge_peer(req) == Some(peer) { self.clear_hedge(req); }
    }
    fn record_rescue(&self) {}
    fn record_hedge(&self) {}
    fn peer_has_piece(&self, peer: PeerKey, piece: PieceIndex) -> bool;
    fn webtor_peer_snap(&self, peer: PeerKey) -> Option<WebtorPeerSnap>;
    fn connected_peer_keys(&self) -> Vec<PeerKey>;
    fn arrival_net(&self) -> Option<ArrivalNet> { None }
    fn learn_arrival(&self, _sample: ArrivalSample, _actual: f64) {}
    fn register_peer_arrival(&self, _peer: PeerKey, _state: Arc<RwLock<PeerArrivalState>>) {}
    fn peer_arrival(&self, _peer: PeerKey) -> Option<Arc<RwLock<PeerArrivalState>>> { None }

    fn on_metadata_data(&self, piece: u32, total_size: u32, data: Bytes) -> Result<bool>;
    fn metadata_piece(&self, piece: u32) -> Option<(u32, Bytes)>;
    fn metadata_size(&self) -> Option<u32>;
    fn has_metadata(&self) -> bool;
    /// Ensure ut_metadata total size is known before requesting pieces.
    fn ensure_metadata_size(&self, size: u32) -> Result<()> {
        let _ = size;
        Ok(())
    }
    /// Pieces still needed from this peer (excludes peer-local in-flight).
    fn metadata_pieces_to_request(&self, peer: &crate::metadata::PeerMetadataState) -> Vec<u32> {
        let _ = peer;
        Vec::new()
    }
    fn ingest_pex(&self, from: SocketAddr, msg: &PexMsg) -> Result<()>;
    /// Record a live conn for outbound PEX (Go `pex.Add`).
    fn pex_note_connected(&self, _addr: SocketAddr, _encrypted: bool, _utp: bool) {}
    /// Record a disconnect for outbound PEX (Go `pex.Drop`).
    fn pex_note_closed(&self, _addr: SocketAddr, _encrypted: bool, _utp: bool) {}
    /// Build next ut_pex payload; updates `cursor`.
    fn pex_genmsg(&self, cursor: &mut Option<crate::pex::PexCursor>) -> Option<PexMsg> {
        let _ = cursor;
        None
    }
    fn add_dht_node(&self, addr: SocketAddr);
    fn on_peer_listen_port(&self, peer_addr: SocketAddr, port: u16);

    fn choke_config(&self) -> ChokeConfig;
    /// Request-scheduler invalidation: wakes every peer, the dialer and readers.
    fn bump_change(&self);
    /// Data/progress changed without creating request work: wakes readers only.
    fn notify_progress(&self) {
        self.bump_change();
    }
    /// Reschedule one other connection (e.g. to cancel a request it no longer owns).
    fn wake_peer(&self, _peer: PeerKey) {}
    fn on_chunk_received(&self, peer: PeerKey, req: RequestIndex, len: usize);

    /// LTEP `v` field (Go `ExtendedHandshakeClientVersion`).
    fn extended_handshake_client_version(&self) -> String {
        "Webtor 2.0".to_string()
    }
    /// LTEP `p` / listen port for incoming peers.
    fn listen_port(&self) -> u16 {
        0
    }
    /// When true, omit `ut_pex` from the LTEP `m` dict.
    fn pex_disabled(&self) -> bool {
        false
    }
    /// LTEP `reqq` we advertise (Go `localClientReqq` = 1024).
    fn local_reqq(&self) -> u32 {
        LOCAL_CLIENT_REQQ
    }
    /// LTEP `encryption` preference bit.
    fn prefers_encryption(&self) -> bool {
        true
    }
    /// BEP 5 Port message value when both sides support DHT.
    fn dht_port(&self) -> Option<u16> {
        None
    }
}

// ---------------------------------------------------------------- RTT / legacy PeerConn

pub struct RttEstimator {
    pub probe: Option<RequestIndex>,
    pub probe_at: Option<Instant>,
    pub estimate: Duration,
    pub probe_queue_wait: Duration,
}

impl Default for RttEstimator {
    fn default() -> Self {
        Self {
            probe_queue_wait: Duration::ZERO,
            probe: None,
            probe_at: None,
            // Go starts at 0: no playback pipeline cap until a real sample.
            estimate: Duration::ZERO,
        }
    }
}

impl RttEstimator {
    pub fn on_request_sent(&mut self, req: RequestIndex) {
        self.on_request_sent_behind(req, Duration::ZERO);
    }

    /// Probe a request queued behind others; `queue_wait` is the time those
    /// are expected to take. A full pipeline never empties, so waiting for an
    /// idle peer would freeze the estimate at its first sample.
    pub fn on_request_sent_behind(&mut self, req: RequestIndex, queue_wait: Duration) {
        if self.probe_at.is_none()
            || self
                .probe_at
                .map(|t| t.elapsed() > Duration::from_secs(10))
                .unwrap_or(false)
        {
            self.probe = Some(req);
            self.probe_at = Some(Instant::now());
            self.probe_queue_wait = queue_wait;
        }
    }

    pub fn on_request_received(&mut self, req: RequestIndex) {
        if let (Some(probe), Some(sent_at)) = (self.probe, self.probe_at) {
            if req == probe {
                let sample = sent_at.elapsed().saturating_sub(self.probe_queue_wait);
                self.probe = None;
                self.probe_at = None;
                if sample.is_zero() {
                    return;
                }
                if self.estimate.is_zero() {
                    self.estimate = sample;
                } else {
                    let diff = (sample.as_secs_f64() - self.estimate.as_secs_f64()) / 8.0;
                    self.estimate =
                        Duration::from_secs_f64((self.estimate.as_secs_f64() + diff).max(0.001));
                }
            }
        }
    }
}

pub struct PeerConnInner {
    pub remote_addr: SocketAddr,
    pub outgoing: bool,
    pub peer_listen_port: u16,
    pub peer_id: Option<PeerId>,
    pub client_name: String,
    pub transport: Transport,
    pub source: PeerSource,
    pub encrypted: bool,
    pub connected_at: Instant,

    pub peer_choking: bool,
    pub am_choking: bool,
    pub peer_interested: bool,
    pub am_interested: bool,

    pub peer_pieces: HashSet<PieceIndex>,
    pub peer_sent_have_all: bool,
    pub peer_allowed_fast: HashSet<PieceIndex>,
    /// Peer's reserved handshake bits (Fast / LTEP / DHT).
    pub peer_extension_bits: [u8; 8],

    pub max_requests: usize,
    /// Go `peakRequests` starts at 0 and ramps via `next_peak_requests`.
    pub peak_requests: usize,
    pub outstanding_requests: HashSet<RequestIndex>,
    pub cancelled_requests: HashSet<RequestIndex>,

    pub rtt: RttEstimator,
    /// Go `downloadRate`: useful bytes / time expecting chunks.
    pub download_rate: ExpectingRate,
    pub upload_rate: RateEstimator,
    pub stats: AtomicConnStats,

    pub last_useful_chunk_received: Option<Instant>,
    pub webtor_unchoked_at: Option<Instant>,
    pub webtor_choked_since: Option<Instant>,
    pub last_became_interested: Option<Instant>,
    pub webtor_pipeline_limit: usize,

    pub arrival: ArrivalPeer,
    pub arrival_samples: HashMap<RequestIndex, ArrivalSample>,

    pub peer_ext_ut_metadata: Option<u8>,
    pub peer_ext_ut_pex: Option<u8>,
    pub peer_metadata_size: Option<u32>,

    /// Requests we will accept as hedge answers.
    pub valid_receive_chunks: HashMap<RequestIndex, u32>,
    pub is_snubbed: bool,
    pub need_request_update: bool,
}

impl PeerConnInner {
    /// Go `nominalMaxRequests`, including the current playback cap.
    pub(crate) fn nominal_max_requests(&self) -> usize {
        let base = self
            .peak_requests
            .saturating_mul(2)
            .min(self.max_requests)
            .clamp(1, MAX_LOCAL_TO_REMOTE_REQUESTS);
        if self.webtor_pipeline_limit > 0 {
            base.min(self.webtor_pipeline_limit)
        } else {
            base
        }
    }
}

#[derive(Clone)]
pub struct PeerConn {
    pub inner: Arc<RwLock<PeerConnInner>>,
    pub closed: Arc<AtomicBool>,
    pub write_tx: mpsc::UnboundedSender<Message>,
    pub write_waker: Arc<Notify>,
}

impl PeerConn {
    pub fn new(
        remote_addr: SocketAddr,
        outgoing: bool,
        source: PeerSource,
        write_tx: mpsc::UnboundedSender<Message>,
        write_waker: Arc<Notify>,
    ) -> Self {
        let now = Instant::now();
        Self {
            inner: Arc::new(RwLock::new(PeerConnInner {
                remote_addr,
                outgoing,
                peer_listen_port: 0,
                peer_id: None,
                client_name: String::new(),
                transport: Transport::Tcp,
                source,
                encrypted: false,
                connected_at: now,
                peer_choking: true,
                am_choking: true,
                peer_interested: false,
                am_interested: false,
                peer_pieces: HashSet::new(),
                peer_sent_have_all: false,
                peer_allowed_fast: HashSet::new(),
                peer_extension_bits: [0u8; 8],
                max_requests: DEFAULT_PEER_MAX_REQUESTS,
                // Go ramps from an empty queue (nominal capacity starts at one).
                peak_requests: 0,
                outstanding_requests: HashSet::new(),
                cancelled_requests: HashSet::new(),
                rtt: RttEstimator::default(),
                download_rate: ExpectingRate::new(),
                upload_rate: RateEstimator::new(),
                stats: AtomicConnStats::default(),
                last_useful_chunk_received: None,
                webtor_unchoked_at: None,
                webtor_choked_since: Some(now),
                last_became_interested: None,
                webtor_pipeline_limit: 0,
                arrival: ArrivalPeer::default(),
                arrival_samples: HashMap::new(),
                peer_ext_ut_metadata: None,
                peer_ext_ut_pex: None,
                peer_metadata_size: None,
                valid_receive_chunks: HashMap::new(),
                is_snubbed: false,
                need_request_update: false,
            })),
            closed: Arc::new(AtomicBool::new(false)),
            write_tx,
            write_waker,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.write_waker.notify_waiters();
    }

    pub fn send_message(&self, msg: Message) {
        if !self.is_closed() {
            let _ = self.write_tx.send(msg);
            self.write_waker.notify_one();
        }
    }

    pub fn cache_dial_addr(&self) -> Option<SocketAddr> {
        let inner = self.inner.read();
        if inner.outgoing {
            Some(inner.remote_addr)
        } else if inner.peer_listen_port > 0 {
            Some(SocketAddr::new(
                inner.remote_addr.ip(),
                inner.peer_listen_port,
            ))
        } else {
            None
        }
    }

    pub fn pending_requests_count(&self) -> usize {
        let inner = self.inner.read();
        let mut count = inner.outstanding_requests.len();
        if !current_seek_tuning().full_pipeline {
            count += inner.cancelled_requests.len();
        }
        count
    }

    pub fn expected_arrival(
        &self,
        chunk_size: usize,
        use_learned: bool,
        arrival_net: Option<&crate::arrival::ArrivalNet>,
    ) -> f64 {
        let inner = self.inner.read();
        if use_learned {
            if let Some(net) = arrival_net {
                if let Some(x) =
                    self.arrival_inputs_locked(&inner, inner.outstanding_requests.len(), chunk_size)
                {
                    return net.predict(x);
                }
            }
        }
        expected_arrival_secs(
            inner.download_rate.rate(),
            inner.rtt.estimate,
            inner.outstanding_requests.len() as u32,
            chunk_size as u32,
        )
    }

    fn arrival_inputs_locked(
        &self,
        inner: &PeerConnInner,
        queued: usize,
        chunk_size: usize,
    ) -> Option<[f64; ARRIVAL_FEATURES]> {
        inner.arrival.inputs(inner.download_rate.rate(), inner.rtt.estimate,
            queued, chunk_size, if inner.peer_choking { None } else { inner.webtor_unchoked_at })
    }

    pub fn note_request_sent(
        &self,
        req: RequestIndex,
        chunk_size: usize,
        arrival_net: Option<&crate::arrival::ArrivalNet>,
    ) {
        let mut inner = self.inner.write();
        let queued = inner.outstanding_requests.len();
        if queued == 0 {
            // Nothing queued ahead of it: its block measures the round trip.
            inner.rtt.on_request_sent(req);
        }
        inner.outstanding_requests.insert(req);
        inner.cancelled_requests.remove(&req);
        let expecting = inner.am_interested && !inner.peer_choking;
        inner.download_rate.set_expecting(expecting);

        if let Some(net) = arrival_net {
            if let Some(x) =
                self.arrival_inputs_locked(&inner, queued, chunk_size)
            {
                let est = net.predict(x);
                inner.arrival_samples.insert(
                    req,
                    ArrivalSample {
                        sent: Instant::now(),
                        x,
                        estimate: est,
                    },
                );
            }
        }
    }

    pub fn note_block_received(
        &self,
        req: RequestIndex,
        arrival_model: Option<&mut crate::arrival::ArrivalModel>,
    ) {
        let mut inner = self.inner.write();
        inner.outstanding_requests.remove(&req);
        inner.rtt.on_request_received(req);
        let expecting =
            !inner.outstanding_requests.is_empty() && inner.am_interested && !inner.peer_choking;
        inner.download_rate.set_expecting(expecting);
        inner.last_useful_chunk_received = Some(Instant::now());
        inner.arrival.note_block(BLOCK_SIZE as usize);

        if let Some(sample) = inner.arrival_samples.remove(&req) {
            let actual = sample.sent.elapsed().as_secs_f64();
            let err = (sample.estimate - actual).abs();
            inner.arrival.on_block_arrived(err);
            if let Some(model) = arrival_model {
                model.on_block_learned(sample.x, sample.estimate, actual);
            }
        }
    }

    pub fn block_overdue(
        &self,
        req: RequestIndex,
        min_timeout: Duration,
        use_learned: bool,
    ) -> bool {
        self.learned_block_overdue(req, min_timeout, use_learned).unwrap_or(false)
    }

    pub fn learned_block_overdue(&self, req: RequestIndex, min_timeout: Duration,
        use_learned: bool) -> Option<bool> {
        let inner = self.inner.read();
        if use_learned {
            if let Some(sample) = inner.arrival_samples.get(&req) {
                let late = inner.arrival.overdue_duration(sample.estimate, min_timeout);
                return Some(sample.sent.elapsed() >= late);
            }
        }
        None
    }

    pub fn peer_conn_info(&self) -> PeerConnInfo {
        let inner = self.inner.read();
        PeerConnInfo {
            remote_addr: inner.remote_addr,
            dial_addr: if inner.outgoing {
                Some(inner.remote_addr)
            } else {
                None
            },
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
            peer_completion: 0.0,
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
    }
}

// ---------------------------------------------------------------- ConnHandler

/// Max in-flight ut_metadata piece requests per peer. High enough to fill
/// typical magnet info (~64 KiB pieces) in one burst; timeout retry refills.
const METADATA_OUTSTANDING_PER_PEER: usize = 128;

/// ConnHandler-backed peer state machine.
pub struct PeerConnHandler<T: PeerTorrent> {
    torrent: Arc<T>,
    state: PeerConnInner,
    closed: Arc<AtomicBool>,
    our_ext_metadata: u8,
    our_ext_pex: u8,
    peer_metadata: PeerMetadataState,
    pex_cursor: Option<PexCursor>,
    last_pex_sent: Option<Instant>,
    pex_listed: bool,
    cancelled_at: HashMap<RequestIndex, Instant>,
    arrival: Arc<RwLock<PeerArrivalState>>,
    wanted_cache: std::cell::Cell<Option<(Instant, bool)>>,
    /// When a refill last found nothing to request.
    idle_refill_at: Option<Instant>,
    last_snapshot: Instant,
    last_warm: Instant,
    /// Block asked of a new peer to measure it, and when.
    speed_probe: Option<(RequestIndex, Instant)>,
    speed_probed: bool,
}

/// Run storage work that may wait on disk (evicting or saving a cache slot
/// through Android's FUSE layer). Go parks only the goroutine of the peer
/// that wrote; on a tokio worker every connection scheduled there would
/// stall, so the worker hands its other tasks to another thread meanwhile.
fn blocking_io<R>(f: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

fn sorted_requests(set: &HashSet<RequestIndex>) -> Vec<RequestIndex> {
    let mut v: Vec<_> = set.iter().copied().collect();
    v.sort_unstable();
    v
}

/// A peer with requests outstanding that delivered nothing for this long
/// releases them (Go relies on balance stealing; this only catches dead peers).
const DEAD_PEER_AFTER: Duration = Duration::from_secs(30);
/// Endgame starts when at most this much of the selection is left.
/// Below this many requests a peer counts as about to run dry.
const BACKGROUND_STARVING_REQUESTS: usize = 16;
pub(crate) const ENDGAME_BYTES: i64 = 16 * 1024 * 1024;

/// Reuse of [`PeerConnHandler::peer_has_wanted_pieces`]; below the 50 ms
/// choke-reconcile wake interval.
const WANTED_PIECES_CACHE: Duration = Duration::from_millis(25);
/// A refill rescans every piece; one that found nothing retries at this pace.
const IDLE_REFILL_EVERY: Duration = Duration::from_millis(250);
/// Received blocks publish the peer's snapshot at this pace; each publish
/// takes a torrent-wide write lock.
const SNAPSHOT_EVERY: Duration = Duration::from_millis(100);
/// How long a useful unchoked peer may sit idle during playback before a
/// warm-up request, so a seek can start on a connection already moving data.
const WARM_PEER_IDLE: Duration = Duration::from_secs(30);
/// Least time between warm-up requests in one torrent: at most 8 KiB/s.
pub(crate) const WARM_TORRENT_GAP_MS: u64 = 2_000;
/// Wake interval of a choked peer we have nothing queued with.
const IDLE_CHOKED_WAKE: Duration = Duration::from_secs(1);

impl<T: PeerTorrent> PeerConnHandler<T> {
    pub fn new(torrent: Arc<T>, handle: &ConnHandle) -> Self {
        let now = Instant::now();
        Self {
            torrent,
            state: PeerConnInner {
                remote_addr: handle.remote_addr(),
                outgoing: handle.is_outgoing(),
                peer_listen_port: 0,
                peer_id: handle.peer_id(),
                client_name: String::new(),
                transport: handle.transport(),
                source: handle.source(),
                encrypted: handle.is_encrypted(),
                connected_at: now,
                peer_choking: true,
                am_choking: true,
                peer_interested: false,
                am_interested: false,
                peer_pieces: HashSet::new(),
                peer_sent_have_all: false,
                peer_allowed_fast: HashSet::new(),
                peer_extension_bits: handle.peer_extension_bits(),
                max_requests: DEFAULT_PEER_MAX_REQUESTS,
                // Go ramps from an empty queue (nominal capacity starts at one).
                peak_requests: 0,
                outstanding_requests: HashSet::new(),
                cancelled_requests: HashSet::new(),
                rtt: RttEstimator::default(),
                download_rate: ExpectingRate::new(),
                upload_rate: RateEstimator::new(),
                stats: AtomicConnStats::default(),
                last_useful_chunk_received: None,
                webtor_unchoked_at: None,
                webtor_choked_since: Some(now),
                last_became_interested: None,
                webtor_pipeline_limit: 0,
                arrival: ArrivalPeer::default(),
                arrival_samples: HashMap::new(),
                peer_ext_ut_metadata: None,
                peer_ext_ut_pex: None,
                peer_metadata_size: None,
                valid_receive_chunks: HashMap::new(),
                is_snubbed: false,
                need_request_update: true,
            },
            closed: Arc::new(AtomicBool::new(false)),
            our_ext_metadata: DEFAULT_UT_METADATA_ID,
            our_ext_pex: DEFAULT_UT_PEX_ID,
            peer_metadata: PeerMetadataState::new(),
            pex_cursor: None,
            last_pex_sent: None,
            pex_listed: false,
            cancelled_at: HashMap::new(),
            arrival: Arc::new(RwLock::new(PeerArrivalState::default())),
            wanted_cache: std::cell::Cell::new(None),
            idle_refill_at: None,
            last_snapshot: Instant::now(),
            last_warm: now,
            speed_probe: None,
            speed_probed: false,
        }
    }

    fn maybe_send_pex(&mut self, handle: &ConnHandle) {
        if self.torrent.pex_disabled() {
            return;
        }
        let Some(pex_id) = self.state.peer_ext_ut_pex else {
            return;
        };
        let now = Instant::now();
        // Go Share: non-empty → sched pexInterval; empty → sched pexRetryDelay.
        // last_pex_sent None allows the first attempt immediately (AfterFunc(0)).
        let wait = if self.pex_cursor.is_some() {
            PEX_INTERVAL
        } else {
            PEX_RETRY_DELAY
        };
        if self
            .last_pex_sent
            .is_some_and(|t| now.saturating_duration_since(t) < wait)
        {
            return;
        }
        let Some(msg) = self.torrent.pex_genmsg(&mut self.pex_cursor) else {
            self.last_pex_sent = Some(now);
            self.pex_cursor = None;
            return;
        };
        if msg.is_empty() {
            self.last_pex_sent = Some(now);
            // Go leaves the cursor unchanged when Len==0 so the next try is still
            // a full snapshot once live conns appear in PexState.
            self.pex_cursor = None;
            return;
        }
        self.send(handle, msg.to_message(pex_id));
        self.last_pex_sent = Some(now);
    }

    /// Request missing ut_metadata pieces with timeout retry (anacrolix-style).
    fn request_metadata_pieces(&mut self, handle: &ConnHandle) {
        if self.torrent.has_metadata() {
            return;
        }
        let Some(m_id) = self.state.peer_ext_ut_metadata else {
            return;
        };
        let size = self.state.peer_metadata_size.unwrap_or(0);
        if size == 0 {
            return;
        }
        if self.torrent.ensure_metadata_size(size).is_err() {
            return;
        }
        let now = Instant::now();
        self.peer_metadata
            .expire_timed_out(now, METADATA_REQUEST_TIMEOUT);
        let slots = METADATA_OUTSTANDING_PER_PEER.saturating_sub(self.peer_metadata.outstanding());
        if slots == 0 {
            return;
        }
        let needed = self.torrent.metadata_pieces_to_request(&self.peer_metadata);
        for piece in needed.into_iter().take(slots) {
            self.peer_metadata.mark_requested_at(piece, now);
            let req = MetadataMsg::request(piece);
            self.send(handle, req.to_message(m_id));
        }
    }

    pub fn key(&self) -> PeerKey {
        self.torrent.peer_key(self.state.remote_addr)
    }

    /// Go `sendInitialMessages` (Transmission order): LTEP, HaveNone/HaveAll/bitfield, optional Port.
    /// Does **not** Unchoke or fill requests — Go leaves that to `upload` / peer-driven
    /// `updateRequests` after the remote's bitfield/unchoke.
    pub fn start(&mut self, handle: &ConnHandle) {
        self.torrent.register_peer_arrival(self.key(), self.arrival.clone());
        let peer_bits = PeerExtensionBits(self.state.peer_extension_bits);
        if peer_bits.supports_extended() {
            let mut ext = ExtendedHandshake::default();
            ext.m
                .insert(EXTENSION_NAME_METADATA.to_string(), self.our_ext_metadata);
            if !self.torrent.pex_disabled() {
                ext.m
                    .insert(EXTENSION_NAME_PEX.to_string(), self.our_ext_pex);
            }
            ext.v = Some(self.torrent.extended_handshake_client_version());
            // Go `p` has omitempty — never advertise listen port 0.
            let listen = self.torrent.listen_port();
            if listen > 0 {
                ext.p = Some(listen);
            }
            ext.yourip = Some(handle.remote_addr().ip());
            ext.reqq = Some(self.torrent.local_reqq());
            ext.metadata_size = self.torrent.metadata_size();
            // Go ExtendedHandshakeMessage has no upload_only field — omit it.
            ext.encryption = Some(self.torrent.prefers_encryption() || handle.is_encrypted());
            self.send(
                handle,
                Message::Extended {
                    id: EXT_HANDSHAKE_ID,
                    payload: ext.encode(),
                },
            );
        }

        let count = self.torrent.num_pieces();
        let have_all = count > 0 && (0..count).all(|i| self.torrent.piece_complete(i));
        let have_any = (0..count).any(|i| self.torrent.piece_complete(i));
        if peer_bits.supports_fast() {
            if have_all {
                self.send(handle, Message::HaveAll);
            } else if !have_any {
                self.send(handle, Message::HaveNone);
            } else if count > 0 {
                self.send_bitfield(handle, count);
            }
        } else if have_any {
            // Go `postBitfield`: skip when we have nothing (no empty bitfield).
            self.send_bitfield(handle, count);
        }

        if peer_bits.supports_dht() {
            if let Some(port) = self.torrent.dht_port() {
                self.send(handle, Message::Port(port));
            }
        }

        self.torrent.update_peer_snapshot(self.peer_conn_info());
    }

    /// Go `peerHasWantedPieces`: peer holds something in our pending set (tit-for-tat).
    /// Go reads a maintained pending-piece bitmap. This runs on every message
    /// and wake, so the torrent scan is reused for a short interval.
    fn peer_has_wanted_pieces(&self) -> bool {
        if !self.torrent.have_info() {
            return !self.state.peer_pieces.is_empty();
        }
        let now = Instant::now();
        if let Some((at, wanted)) = self.wanted_cache.get() {
            if now.saturating_duration_since(at) < WANTED_PIECES_CACHE {
                return wanted;
            }
        }
        let wanted = if self.state.peer_sent_have_all {
            self.torrent.num_pieces() > 0 && self.torrent.any_wanted_incomplete()
        } else {
            self.torrent.any_wanted_among(&self.state.peer_pieces)
        };
        self.wanted_cache.set(Some((now, wanted)));
        wanted
    }

    /// Go `upload` choke/unchoke reconciliation via `uploadAllowed`.
    fn reconcile_choke(&mut self, handle: &ConnHandle) {
        let cfg = self.torrent.choke_config();
        let input = PeerChokeInput {
            id: self.key(),
            interested: self.state.peer_interested,
            currently_choking: self.state.am_choking,
            download_rate: self.state.download_rate.rate(),
            upload_rate: self.state.upload_rate.rate(),
            bytes_downloaded: self.state.stats.snapshot().bytes_read_useful_data,
            bytes_uploaded: self.state.stats.snapshot().bytes_written_data,
            is_snubbed: self.state.is_snubbed,
        };
        let allowed = upload_allowed(&input, &cfg, self.peer_has_wanted_pieces());
        let should_choke = !allowed;
        if self.state.am_choking != should_choke {
            self.state.am_choking = should_choke;
            self.send(
                handle,
                if should_choke {
                    Message::Choke
                } else {
                    Message::Unchoke
                },
            );
        }
    }

    fn send_bitfield(&self, handle: &ConnHandle, count: u32) {
        let mut bits = vec![0u8; (count as usize).div_ceil(8)];
        for piece in 0..count {
            if self.torrent.piece_complete(piece) {
                bits[piece as usize / 8] |= 0x80 >> (piece % 8);
            }
        }
        self.send(handle, Message::Bitfield(Bytes::from(bits)));
    }

    pub fn peer_conn_info(&self) -> PeerConnInfo {
        PeerConnInfo {
            remote_addr: self.state.remote_addr,
            dial_addr: if self.state.outgoing {
                Some(self.state.remote_addr)
            } else {
                None
            },
            peer_id: self.state.peer_id,
            client_name: self.state.client_name.clone(),
            transport: self.state.transport,
            source: self.state.source,
            encrypted: self.state.encrypted,
            outgoing: self.state.outgoing,
            peer_choking: self.state.peer_choking,
            am_choking: self.state.am_choking,
            peer_interested: self.state.peer_interested,
            am_interested: self.state.am_interested,
            conn_stats: self.state.stats.snapshot(),
            download_rate: self.state.download_rate.rate(),
            upload_rate: self.state.upload_rate.rate(),
            peer_completion: self.peer_completion(),
            pending_requests: self.state.outstanding_requests.len(),
            peer_max_requests: self.state.max_requests,
            peak_requests: self.state.peak_requests,
            pipeline_limit: self.state.webtor_pipeline_limit,
            nominal_max_requests: self.nominal_max(),
            rtt: self.state.rtt.estimate,
            queued: self.state.outstanding_requests.len(),
            cancelled: self.state.cancelled_requests.len(),
            connected_for: self.state.connected_at.elapsed(),
            last_useful_data: self.state.last_useful_chunk_received.map(|t| t.elapsed()),
        }
    }

    fn peer_completion(&self) -> f64 {
        if self.state.peer_sent_have_all {
            return 1.0;
        }
        let n = self.torrent.num_pieces();
        if n == 0 {
            return 0.0;
        }
        self.state.peer_pieces.len() as f64 / f64::from(n)
    }

    fn send(&self, handle: &ConnHandle, msg: Message) {
        let _ = handle.send_message(msg);
    }

    /// Cancel the local request without erasing an assignment stolen by another peer.
    fn cancel_outstanding(&mut self, handle: &ConnHandle, req: RequestIndex) {
        if !self.state.outstanding_requests.remove(&req) { return; }
        wt_types::probe::add(wt_types::probe::CANCELS, 1);
        let coord = block_coord_from_req_index(req, self.torrent.piece_length(), self.torrent.total_length());
        self.send(handle, Message::Cancel { index: coord.piece, begin: coord.offset, length: coord.length.max(1) });
        let bits = PeerExtensionBits(self.state.peer_extension_bits);
        let rejects_cancels = bits.supports_fast() && self.state.peer_id.is_none_or(|id|
            !id.starts_with(b"-TR") || id[7] != b'-' || id[3] >= b'4');
        if rejects_cancels {
            self.state.cancelled_requests.insert(req);
        }
        self.cancelled_at.insert(req, Instant::now());
        let owner = self.torrent.requesting_peer(req);
        self.torrent.release_hedge(req, self.key());
        if owner == Some(self.key()) {
            self.torrent.release_request(req, self.key());
        }
        if (owner.is_none() || owner == Some(self.key())) && self.torrent.hedge_peer(req).is_none() {
            self.torrent.clear_chunk_requested(req);
        }
    }

    /// Drop outstanding requests that have waited too long so the pipeline can refill.
    fn expire_stale_requests(&mut self, handle: &ConnHandle) {
        let now = Instant::now();
        let rtt = self.state.rtt.estimate.max(Duration::from_millis(200));
        let limit = (rtt * 8).max(Duration::from_secs(4));
        // Some Fast peers never reject a Cancel. Non-Fast peers are not
        // required to reply at all. Neither may hold pipeline slots forever.
        let expired: Vec<_> = self.cancelled_at.iter()
            .filter(|(_, when)| now.saturating_duration_since(**when) >= limit)
            .map(|(&req, _)| req).collect();
        for req in expired {
            self.cancelled_at.remove(&req);
            self.state.cancelled_requests.remove(&req);
            if !self.state.outstanding_requests.contains(&req) {
                self.state.valid_receive_chunks.remove(&req);
                self.arrival.write().samples.remove(&req);
            }
            self.state.need_request_update = true;
        }
        // Go never cancels outstanding requests on a timer. A deep queue to a
        // peer serving at its own pace legitimately takes longer than any
        // fixed age; cancelling those re-requested blocks that were still on
        // their way, and the late copies were discarded (measured: ~47% of a
        // plain download wasted). Slow holders lose blocks through balance
        // stealing instead. Only a peer that has delivered nothing for
        // DEAD_PEER_AFTER releases its queue.
        let silent = self.state.last_useful_chunk_received
            .is_none_or(|t| now.saturating_duration_since(t) >= DEAD_PEER_AFTER);
        if !silent {
            return;
        }
        let stale: Vec<_> = self
            .state
            .outstanding_requests
            .iter()
            .copied()
            .filter(|r| {
                self.torrent
                    .request_when(*r)
                    .map(|t| now.saturating_duration_since(t) > DEAD_PEER_AFTER)
                    .unwrap_or(false)
            })
            .collect();
        if stale.is_empty() {
            return;
        }
        // Go decrements peak per cancel: a peer that stopped delivering
        // restarts from a small queue.
        wt_types::probe::add(wt_types::probe::STALE_RELEASED, stale.len() as u64);
        self.state.peak_requests = self.state.peak_requests.saturating_sub(stale.len());
        for req in stale {
            self.cancel_outstanding(handle, req);
        }
        self.state.need_request_update = true;
    }

    /// Recompute and apply desired requests (Go `updateRequests`).
    pub fn update_requests(&mut self, handle: &ConnHandle) {
        if self.torrent.closed()
            || self.torrent.data_download_disallowed()
            || !self.torrent.have_info()
            || handle.is_closed()
        {
            return;
        }
        self.state.need_request_update = false;
        // Go cancels the previous owner synchronously when stealing. Our peer
        // handlers own their queues, so reconcile that ownership on their wake.
        let refocus = handle.take_refocus();
        let mut displaced = self.torrent.displaced_requests(&self.state.outstanding_requests, self.key());
        if refocus {
            let (plen, total) = (self.torrent.piece_length(), self.torrent.total_length());
            displaced.extend(self.state.outstanding_requests.iter().copied().filter(|&req| {
                self.torrent.piece_priority(block_coord_from_req_index(req, plen, total).piece)
                    < PiecePriority::Now
            }));
        }
        for req in displaced { self.cancel_outstanding(handle, req); }
        let tuning = self.torrent.seek_tuning();
        let background = self.torrent.background_only();
        // Go `webtorPipelineLimit` during playback; a few seconds of the
        // peer's rate otherwise. Fixed for this refill.
        self.state.webtor_pipeline_limit = crate::picker::compute_peer_pipeline_limit(
            self.state.download_rate.rate(),
            self.state.rtt.estimate,
            !background && self.torrent.request_focus_index().is_some(),
        );
        if !background && self.torrent.request_focus_index().is_some() && (tuning.hol_rescue || tuning.hedge) {
            self.apply_hol_and_hedge(handle, &tuning);
        }
        let req_tuning = RequestTuning::from_seek_tuning(
            tuning.bound_candidates,
            tuning.pipeline_seconds,
            tuning.min_pipeline,
            tuning.pipeline_rtt,
            self.torrent.request_focus_index().is_some(),
        );

        let num_pieces = self.torrent.num_pieces();
        if self.state.peer_sent_have_all
            && num_pieces > 0
            && self.state.peer_pieces.len() < num_pieces as usize
        {
            // HaveAll before info: its availability was counted back then.
            self.state.peer_pieces.extend(0..num_pieces);
            self.torrent.note_peer_pieces(self.key(), &mut (0..num_pieces));
        }
        if background {
            self.refill_background(handle, &tuning);
            return;
        }
        wt_types::probe::add(wt_types::probe::OLD_PICKER, 1);
        let mut has_piece = vec![false; num_pieces as usize];
        let mut allowed = vec![false; num_pieces as usize];
        if self.state.peer_sent_have_all {
            has_piece.fill(true);
        } else {
            for &p in &self.state.peer_pieces {
                if (p as usize) < has_piece.len() {
                    has_piece[p as usize] = true;
                }
            }
        }
        for &p in &self.state.peer_allowed_fast {
            if (p as usize) < allowed.len() {
                allowed[p as usize] = true;
            }
        }

        let peer_snap = DesiredPeerSnapshot {
            peer_id: self.key(),
            has_piece,
            peer_choking: self.state.peer_choking,
            peer_allowed_fast: allowed,
            outstanding: sorted_requests(&self.state.outstanding_requests),
            cancelled: sorted_requests(&self.state.cancelled_requests),
            nominal_max_requests: self.nominal_max() as u32,
        };

        let (snap, order) = self.torrent.desired_request_context_for_peer(&peer_snap);

        let mut desired = compute_desired_requests(&snap, &peer_snap, &order, &req_tuning);
        sort_requests_for_peer(
            &mut desired.request_indexes,
            &snap,
            &peer_snap,
            &desired.piece_states,
        );

        let was_empty = self.state.outstanding_requests.is_empty();
        let before = self.state.outstanding_requests.len() as u32;
        let clock = SystemClock;
        let steal_after = self.torrent.urgent_steal_after();
        let focus = self.torrent.request_focus_index();
        let focus_set = focus.is_some();
        let chunk = self.torrent.chunk_size();
        let plen = self.torrent.piece_length();
        let total = self.torrent.total_length();

        // Interest — sticky like Go `allowSendNotInterested` +
        // `webtorHasPlayingPieceWeLack`. qBittorrent punishes NotInterested
        // while we are still streaming incomplete data (same peer 107.175
        // historically dropped to ~49 KiB on Rust after brief NotInterested).
        let mut interested = desired.interested;
        if !interested && self.state.am_interested {
            let have_all = (0..num_pieces).all(|i| self.torrent.piece_complete(i));
            if !have_all {
                if self.state.peer_sent_have_all || self.state.peer_pieces.is_empty() {
                    // Have-all or bitfield not yet known → keep interested.
                    interested = true;
                } else {
                    let peer_only_done = self
                        .state
                        .peer_pieces
                        .iter()
                        .all(|&p| self.torrent.piece_complete(p));
                    if !peer_only_done {
                        interested = true;
                    }
                }
            }
        }
        if !interested && !self.torrent.data_download_disallowed() {
            // Go `webtorHasPlayingPieceWeLack`: stay interested while the
            // playhead window still needs pieces this peer can serve.
            if let Some(f) = focus {
                let begin = block_coord_from_req_index(f, plen, total).piece;
                let end = begin.saturating_add(64).min(num_pieces);
                let peer_has_all = self.state.peer_sent_have_all;
                let lacks = |p: u32| !self.torrent.piece_complete(p);
                let playing = if peer_has_all {
                    (begin..end).any(lacks)
                } else {
                    self.state
                        .peer_pieces
                        .iter()
                        .any(|&p| p >= begin && p < end && lacks(p))
                };
                if playing {
                    interested = true;
                }
            }
        }
        if interested != self.state.am_interested {
            self.state.am_interested = interested;
            if interested {
                self.state.last_became_interested = Some(Instant::now());
                self.send(handle, Message::Interested);
            } else {
                self.send(handle, Message::NotInterested);
            }
        }

        let mut added = 0u32;
        let mut claimed_focus = false;
        let mut holder_request_times: HashMap<PeerKey, Vec<Instant>> = HashMap::new();
        let mut yields_left = 10u32;
        let mut yielded_pieces = HashSet::new();
        let mut seek_peers = if focus.is_some() { self.seek_peer_snapshots() } else { Vec::new() };
        let arrival_net = self.torrent.arrival_net();
        let learned = arrival_net.as_ref().map(|net| net as &dyn crate::webtor::ArrivalPredictor);
        // Endgame: little is left and every block this peer could fetch is
        // already on its way from another peer. A second copy from this idle
        // peer means one slow holder cannot hold up the end; the first copy
        // wins and the other request is cancelled (hedge bookkeeping).
        // Playback has its own head-of-line hedging.
        let me = self.key();
        let endgame = focus.is_none()
            && i64::from(snap.wanted_pieces).saturating_mul(snap.piece_length.max(1)) <= ENDGAME_BYTES
            && !desired.request_indexes.is_empty()
            && desired.request_indexes.iter().all(|r| self.state.outstanding_requests.contains(r)
                || snap.assignments.get(r).is_some_and(|a| a.peer != me));

        for r in desired.request_indexes {
            let pending = pending_requests(
                &tuning,
                self.state.outstanding_requests.len() as u32,
                self.state.cancelled_requests.len() as u32,
            );
            if pending >= self.nominal_max() as u32 {
                break;
            }
            if self.state.outstanding_requests.contains(&r) {
                continue;
            }
            let piece = snap
                .piece_of_request
                .get(&r)
                .copied()
                .unwrap_or_else(|| block_coord_from_req_index(r, plen, total).piece);
            let prio = self.torrent.piece_priority(piece);

            if let Some(me) = seek_peers.first_mut() {
                me.queued = self.state.outstanding_requests.len() as u32;
                me.cancelled = self.state.cancelled_requests.len() as u32;
                me.arrival_inputs = self.arrival_inputs();
            }
            if yielded_pieces.contains(&piece) { continue; }
            if tuning.slow_peer_yield && yields_left > 0 && prio == PiecePriority::Next
                && self.state.last_useful_chunk_received.is_some()
                && seek_peers.first().is_some_and(|me| crate::webtor::yield_to_faster_peers(
                    me, &seek_peers, piece,
                    PeerTorrentView(&*self.torrent).piece_bytes_left(piece), chunk)) {
                yields_left -= 1;
                yielded_pieces.insert(piece);
                continue;
            }

            // Use the same learned decision as Go. Full peers cannot make us
            // yield, and deciding to yield must not erase the current owner.
            if seek_peers.first().is_some_and(|me| crate::webtor::faster_focus_peer(
                &tuning, steal_after, me, &seek_peers, r, piece, prio, focus, chunk,
                learned, &|peer| peer.arrival_inputs).is_some()) {
                continue;
            }

            let mut stolen_from = None;
            if let Some(holder) = self.torrent.requesting_peer(r) {
                if holder != self.key() {
                    if endgame {
                        if (!self.state.peer_choking || self.state.peer_allowed_fast.contains(&piece))
                            && (self.torrent.claim_hedge(r, me) || self.take_over_hedge(r))
                        {
                            let coord = block_coord_from_req_index(r, plen, total);
                            if !handle.send_message(Message::Request {
                                index: coord.piece,
                                begin: coord.offset,
                                length: coord.length.max(1),
                            }) {
                                self.torrent.clear_hedge(r);
                                break;
                            }
                            self.note_arrival_request(r, arrival_net.as_ref());
                            self.state.outstanding_requests.insert(r);
                            *self.state.valid_receive_chunks.entry(r).or_default() += 1;
                            self.state.cancelled_requests.remove(&r);
                            self.cancelled_at.remove(&r);
                            self.torrent.mark_chunk_requested(r);
                            added += 1;
                        }
                        continue;
                    }
                    let holder_when = self.torrent.request_when(r).unwrap_or_else(Instant::now);
                    let holder_snap = self.torrent.webtor_peer_snap(holder);
                    let holder_uncancelled = holder_snap.as_ref().map(|s| s.queued).unwrap_or(1);
                    let holder_useful = holder_snap.as_ref().and_then(|s| s.last_useful);
                    if !may_take_existing_request(
                        &clock,
                        &tuning,
                        focus_set,
                        steal_after,
                        r,
                        prio,
                        holder_when,
                        self.state.outstanding_requests.len() as u32,
                        self.state.last_useful_chunk_received,
                        holder_uncancelled,
                        holder_useful,
                        focus,
                        false,
                    ) {
                        continue;
                    }
                    // A balance steal of a block the holder has most likely
                    // sent already only duplicates it: both copies arrive and
                    // one is discarded (measured: ~80% of steals during
                    // seeks, a fifth of all data received). Steals of stuck
                    // blocks and of the few blocks the player needs next
                    // still go ahead.
                    let at_playhead = focus.is_some_and(|f| r >= f && r < f + tuning.focus_blocks as u32);
                    if !at_playhead && holder_snap.as_ref().is_some_and(|s| steal_lands_in_flight(
                            holder_request_times.entry(holder)
                                .or_insert_with(|| self.torrent.request_times(holder))
                                .partition_point(|at| *at < holder_when),
                            s.download_rate_bps, s.rtt, chunk))
                        && !should_steal_urgent(&clock, &tuning, steal_after, r, prio, holder_when,
                            self.state.last_useful_chunk_received, holder_useful, focus)
                    {
                        continue;
                    }
                    // Steal: clear assignment; holder cancels on its own refill.
                    self.torrent.release_request(r, holder);
                    stolen_from = Some(holder);
                }
            }

            if self.state.peer_choking && !self.state.peer_allowed_fast.contains(&piece) {
                continue;
            }

            let coord = block_coord_from_req_index(r, plen, total);
            // Peer handlers run concurrently. Reserve before sending so two
            // refill snapshots cannot both overwrite the same block's owner.
            if !self.torrent.claim_request(r, self.key(), Instant::now()) {
                continue;
            }
            let queued = handle.send_message(Message::Request {
                index: coord.piece,
                begin: coord.offset,
                length: coord.length.max(1),
            });
            if !queued {
                self.torrent.release_request(r, self.key());
                break;
            }
            self.note_arrival_request(r, arrival_net.as_ref());
            // The queue ahead is subtracted so the estimate does not grow
            // with it and inflate the playback pipeline sized from it.
            self.probe_round_trip(r);
            claimed_focus |= focus.is_some_and(|f| r >= f && r < f + tuning.focus_blocks as u32);
            self.state.outstanding_requests.insert(r);
            *self.state.valid_receive_chunks.entry(r).or_default() += 1;
            self.state.cancelled_requests.remove(&r);
            self.cancelled_at.remove(&r);
            self.torrent.mark_chunk_requested(r);
            added += 1;
            if let Some(holder) = stolen_from {
                self.torrent.wake_peer(holder);
            }
        }

        if claimed_focus {
            self.torrent.focus_block_claimed();
        }

        let after = self.state.outstanding_requests.len() as u32;
        let computed = next_peak_requests(
            tuning.peak_mode,
            self.state.peak_requests as u32,
            after,
            added,
            was_empty,
        ) as usize;
        // Choke, seek cancellation or a temporarily full hash queue must not
        // restart a productive peer's slow start from a single request.
        self.state.peak_requests = self.state.peak_requests.max(computed);
        self.idle_refill_at = (added == 0).then(Instant::now);
        let _ = before;
    }

    /// Refill for a saved download. Each peer takes the next free blocks it can
    /// serve from the queue all peers share. Nobody takes blocks from anybody:
    /// queues grow with speed, so taking from long queues robbed the fastest
    /// peers. A peer that stops delivering hands its blocks back; the very end
    /// takes second copies so one slow holder cannot hold it up.
    fn refill_background(&mut self, handle: &ConnHandle, tuning: &SeekTuning) {
        let me = self.key();
        let pending = pending_requests(
            tuning,
            self.state.outstanding_requests.len() as u32,
            self.state.cancelled_requests.len() as u32,
        ) as usize;
        let room = self.nominal_max().saturating_sub(pending);
        let pick = self.torrent.background_pick(
            &BackgroundPeer {
                key: me,
                have_all: self.state.peer_sent_have_all,
                pieces: &self.state.peer_pieces,
                allowed_fast: &self.state.peer_allowed_fast,
                choking: self.state.peer_choking,
                cancelled: &self.state.cancelled_requests,
                starving: self.state.outstanding_requests.len() < BACKGROUND_STARVING_REQUESTS,
            },
            room,
        );

        // Interest stays while the peer has anything unfinished we want, even
        // pieces being hashed: dropping it costs our place in its unchoke queue.
        let interested = pick.interested
            || (self.state.am_interested
                && if self.state.peer_sent_have_all {
                    self.torrent.any_wanted_incomplete()
                } else {
                    self.torrent.any_wanted_among(&self.state.peer_pieces)
                });
        if interested != self.state.am_interested {
            self.state.am_interested = interested;
            if interested {
                self.state.last_became_interested = Some(Instant::now());
                self.send(handle, Message::Interested);
            } else {
                self.send(handle, Message::NotInterested);
            }
        }

        let (plen, total) = (self.torrent.piece_length(), self.torrent.total_length());
        let net = self.torrent.arrival_net();
        let was_empty = self.state.outstanding_requests.is_empty();
        let mut added = 0u32;
        for r in pick.free.into_iter().take(room) {
            if self.state.outstanding_requests.contains(&r) || !self.torrent.claim_request(r, me, Instant::now()) {
                continue;
            }
            let coord = block_coord_from_req_index(r, plen, total);
            if !handle.send_message(Message::Request {
                index: coord.piece,
                begin: coord.offset,
                length: coord.length.max(1),
            }) {
                self.torrent.release_request(r, me);
                break;
            }
            self.note_arrival_request(r, net.as_ref());
            self.probe_round_trip(r);
            self.state.outstanding_requests.insert(r);
            *self.state.valid_receive_chunks.entry(r).or_default() += 1;
            self.state.cancelled_requests.remove(&r);
            self.cancelled_at.remove(&r);
            added += 1;
        }
        if added == 0 && pick.wanted_bytes <= ENDGAME_BYTES {
            for r in pick.held.into_iter().take(room) {
                if self.state.outstanding_requests.contains(&r) || !self.torrent.claim_hedge(r, me) {
                    continue;
                }
                let coord = block_coord_from_req_index(r, plen, total);
                if !handle.send_message(Message::Request {
                    index: coord.piece,
                    begin: coord.offset,
                    length: coord.length.max(1),
                }) {
                    self.torrent.clear_hedge(r);
                    break;
                }
                self.note_arrival_request(r, net.as_ref());
                self.state.outstanding_requests.insert(r);
                *self.state.valid_receive_chunks.entry(r).or_default() += 1;
                self.state.cancelled_requests.remove(&r);
                self.cancelled_at.remove(&r);
                added += 1;
            }
        }

        let after = self.state.outstanding_requests.len() as u32;
        let computed = next_peak_requests(
            tuning.peak_mode,
            self.state.peak_requests as u32,
            after,
            added,
            was_empty,
        ) as usize;
        self.state.peak_requests = self.state.peak_requests.max(computed);
        wt_types::probe::add(wt_types::probe::REFILLS, 1);
        wt_types::probe::add(wt_types::probe::PICKED, u64::from(added));
        wt_types::probe::add(wt_types::probe::ROOM, room as u64);
        if added == 0 && room > 0 {
            wt_types::probe::add(wt_types::probe::REFILL_EMPTY, 1);
        }
        // A refill costs little here, so an empty one need not lock the next out.
        self.idle_refill_at = None;
    }

    /// Go `PeerConn.expectingChunks` + `Peer.updateExpectingChunks`: the
    /// download rate only counts time with requests outstanding while we are
    /// interested and the peer lets them through.
    fn update_expecting(&mut self) {
        let expecting = !self.state.outstanding_requests.is_empty()
            && self.state.am_interested
            && (!self.state.peer_choking
                || self.state.outstanding_requests.iter().any(|&r| {
                    let piece = block_coord_from_req_index(
                        r,
                        self.torrent.piece_length(),
                        self.torrent.total_length(),
                    )
                    .piece;
                    self.state.peer_allowed_fast.contains(&piece)
                }));
        self.state.download_rate.set_expecting(expecting);
    }

    fn nominal_max(&self) -> usize {
        self.state.nominal_max_requests()
    }

    fn arrival_inputs(&self) -> Option<[f64; ARRIVAL_FEATURES]> {
        let arrival = self.arrival.read();
        arrival.peer.inputs(self.state.download_rate.rate(), self.state.rtt.estimate,
            self.state.outstanding_requests.len(), self.torrent.chunk_size() as usize,
            arrival.unchoked_at)
    }

    fn note_arrival_request(&self, req: RequestIndex, net: Option<&ArrivalNet>) {
        let Some(net) = net else { return; };
        let Some(x) = self.arrival_inputs() else { return; };
        self.arrival.write().samples.insert(req, ArrivalSample {
            sent: Instant::now(), x, estimate: net.predict(x),
        });
    }

    fn note_arrival_block(&self, req: RequestIndex) {
        let sample = {
            let mut arrival = self.arrival.write();
            arrival.peer.note_block(self.torrent.chunk_size() as usize);
            let sample = arrival.samples.remove(&req);
            if let Some(sample) = sample {
                let error = (sample.estimate - sample.sent.elapsed().as_secs_f64()).abs();
                arrival.peer.on_block_arrived(error);
            }
            sample
        };
        // Never hold a peer-state lock while training the shared model.
        if let Some(sample) = sample {
            self.torrent.learn_arrival(sample, sample.sent.elapsed().as_secs_f64());
        }
    }

    /// Ask a newly unchoked peer that got no real work for one block, just to
    /// time it. Peers with no measured speed cannot be ranked, so straight
    /// after opening any of them could end up holding the playhead.
    fn maybe_probe_speed(&mut self, handle: &ConnHandle) {
        if self.speed_probed
            || self.state.peer_choking
            || !self.state.outstanding_requests.is_empty()
            || self.state.last_useful_chunk_received.is_some()
            || self.state.download_rate.rate() > 0.0
            || self.torrent.data_download_disallowed()
            || self.torrent.is_seeding()
        {
            return;
        }
        let (plen, total) = (self.torrent.piece_length(), self.torrent.total_length());
        let focus_piece = self.torrent.request_focus_index()
            .map(|focus| block_coord_from_req_index(focus, plen, total).piece);
        let piece = if self.state.peer_sent_have_all {
            focus_piece.or(Some(0))
        } else {
            focus_piece.filter(|p| self.state.peer_pieces.contains(p))
                .or_else(|| self.state.peer_pieces.iter().next().copied())
        };
        let Some(piece) = piece else { return };
        if piece >= self.torrent.num_pieces() {
            return;
        }
        let req = req_index_from_piece_and_offset(piece, 0, plen);
        let coord = block_coord_from_req_index(req, plen, total);
        if coord.length == 0 {
            return;
        }
        self.speed_probed = true;
        self.speed_probe = Some((req, Instant::now()));
        handle.send_message(Message::Request { index: coord.piece, begin: coord.offset, length: coord.length });
    }

    /// Ask an idle, proven peer for one block near the playhead. Nothing tracks
    /// the request, so the answer is dropped in `handle_piece` as unexpected:
    /// this buys a warm connection, not data.
    fn maybe_warm(&mut self, handle: &ConnHandle) {
        if self.state.peer_choking
            || !self.state.am_interested
            || !self.state.outstanding_requests.is_empty()
            || self.last_warm.elapsed() < WARM_PEER_IDLE
            || self.state.last_useful_chunk_received.is_none_or(|at| at.elapsed() < WARM_PEER_IDLE)
        {
            return;
        }
        let Some(focus) = self.torrent.request_focus_index() else { return };
        let coord = block_coord_from_req_index(focus, self.torrent.piece_length(), self.torrent.total_length());
        if coord.length == 0
            || !(self.state.peer_sent_have_all || self.state.peer_pieces.contains(&coord.piece))
            || !self.torrent.may_warm()
        {
            return;
        }
        self.last_warm = Instant::now();
        handle.send_message(Message::Request { index: coord.piece, begin: coord.offset, length: coord.length });
    }

    fn after_message(&mut self, handle: &ConnHandle, is_block: bool) {
        if self.state.need_request_update {
            self.update_requests(handle);
        }
        self.update_expecting();
        if !is_block || self.last_snapshot.elapsed() >= SNAPSHOT_EVERY {
            self.last_snapshot = Instant::now();
            self.torrent.update_peer_snapshot(self.peer_conn_info());
        }
    }

    /// Go `isLowOnRequests`: normal downloads refill an empty queue;
    /// playback may opt into fractional/half-pipeline refills.
    /// Low on requests, and not just refilled with nothing to request.
    fn wants_refill(&self) -> bool {
        self.is_low_on_requests()
            && self.idle_refill_at.is_none_or(|at| at.elapsed() >= IDLE_REFILL_EVERY)
    }

    fn probe_round_trip(&mut self, req: RequestIndex) {
        let ahead = self.state.outstanding_requests.len();
        if ahead == 0 {
            self.state.rtt.on_request_sent(req);
            return;
        }
        let rate = self.state.download_rate.rate();
        if rate > 0.0 {
            let bytes = ahead as f64 * f64::from(self.torrent.chunk_size());
            self.state.rtt.on_request_sent_behind(req, Duration::from_secs_f64(bytes / rate));
        }
    }

    /// Endgame: one hedge per block, so the first peer to refill would keep
    /// every copy and leave the rest idle. A much faster peer takes it over;
    /// the old hedger cancels when woken.
    fn take_over_hedge(&self, req: RequestIndex) -> bool {
        let me = self.key();
        let Some(other) = self.torrent.hedge_peer(req).filter(|&other| other != me) else {
            return false;
        };
        let other_rate = self.torrent.webtor_peer_snap(other).map_or(0.0, |s| s.download_rate_bps);
        if self.state.download_rate.rate() <= other_rate * 2.0 {
            return false;
        }
        self.torrent.release_hedge(req, other);
        if !self.torrent.claim_hedge(req, me) {
            return false;
        }
        self.torrent.wake_peer(other);
        true
    }

    fn is_low_on_requests(&self) -> bool {
        let tuning = self.torrent.seek_tuning();
        let pending = self.state.outstanding_requests.len();
        if tuning.refill_at > 0.0 && self.torrent.request_focus_index().is_some() {
            pending <= (self.nominal_max() as f64 * tuning.refill_at) as usize
        } else if tuning.full_pipeline {
            pending <= self.nominal_max() / 2
        } else {
            pending == 0 && self.state.cancelled_requests.is_empty()
        }
    }

    fn seek_peer_snapshots(&self) -> Vec<HolPeerSnap<T>> {
        let mut peers: Vec<HolPeerSnap<T>> = Vec::new();
        peers.push(HolPeerSnap {
            key: self.key(),
            download_rate_bps: self.state.download_rate.rate(),
            rtt: self.state.rtt.estimate,
            queued: self.state.outstanding_requests.len() as u32,
            cancelled: self.state.cancelled_requests.len() as u32,
            nominal_max: self.nominal_max() as u32,
            peer_choking: self.state.peer_choking,
            closed: self.closed.load(Ordering::Relaxed),
            last_useful: self.state.last_useful_chunk_received,
            torrent: Arc::clone(&self.torrent),
            valid_receive: self.state.valid_receive_chunks.clone(),
            pieces: HashSet::new(),
            have_all: self.state.peer_sent_have_all,
            lookup_torrent: true,
            arrival_inputs: self.arrival_inputs(),
            arrival: Some(self.arrival.clone()),
        });
        for pk in self.torrent.connected_peer_keys() {
            if pk == self.key() {
                continue;
            }
            if let Some(s) = self.torrent.webtor_peer_snap(pk) {
                let arrival = self.torrent.peer_arrival(pk);
                let arrival_inputs = arrival.as_ref().and_then(|state| {
                    let state = state.read();
                    state.peer.inputs(s.download_rate_bps, s.rtt, s.queued as usize,
                        self.torrent.chunk_size() as usize, state.unchoked_at)
                });
                peers.push(HolPeerSnap {
                    key: s.key,
                    download_rate_bps: s.download_rate_bps,
                    rtt: s.rtt,
                    queued: s.queued,
                    cancelled: s.cancelled,
                    nominal_max: s.nominal_max,
                    peer_choking: s.peer_choking,
                    closed: s.closed,
                    last_useful: s.last_useful,
                    torrent: Arc::clone(&self.torrent),
                    valid_receive: HashMap::new(),
                    pieces: HashSet::new(),
                    have_all: false,
                    lookup_torrent: true,
                    arrival_inputs,
                    arrival,
                });
            }
        }
        peers
    }

    /// Apply HOL rescue / hedge moves that involve this peer (Go nudge path).
    fn apply_hol_and_hedge(&mut self, handle: &ConnHandle, tuning: &SeekTuning) {
        let clock = SystemClock;
        let torrent = self.torrent.clone();
        let view = PeerTorrentView(&*torrent);
        let peers = self.seek_peer_snapshots();
        let net = self.torrent.arrival_net();
        let learned = net.as_ref().map(|net| net as &dyn crate::webtor::ArrivalPredictor);
        let features_for = |p: &HolPeerSnap<T>| p.arrival_inputs;
        let learned_od = |p: &HolPeerSnap<T>, r: RequestIndex| -> Option<bool> {
            if !tuning.learned_arrival { return None; }
            p.arrival.as_ref()?.read().overdue(r, Duration::from_millis(100))
        };

        if tuning.hol_rescue {
            let moves = rescue_decisions(&clock, tuning, &view, &peers, learned, &features_for);
            let me = self.key();
            let plen = self.torrent.piece_length();
            let total = self.torrent.total_length();
            for m in moves {
                if m.from == me && self.state.outstanding_requests.contains(&m.req) {
                    self.cancel_outstanding(handle, m.req);
                    self.torrent.record_rescue();
                } else if m.to == me
                    && !self.state.outstanding_requests.contains(&m.req)
                    && !self.state.peer_choking
                {
                    let coord = block_coord_from_req_index(m.req, plen, total);
                    if self.state.peer_sent_have_all
                        || self.state.peer_pieces.contains(&coord.piece)
                        || self.state.peer_allowed_fast.contains(&coord.piece)
                    {
                        if self.state.outstanding_requests.len() >= self.nominal_max() { break; }
                        self.torrent.release_request(m.req, m.from);
                        if !self.torrent.claim_request(m.req, me, Instant::now()) { continue; }
                        if m.from != me { self.torrent.wake_peer(m.from); }
                        if !handle.send_message(Message::Request {
                                index: coord.piece,
                                begin: coord.offset,
                                length: coord.length.max(1),
                            }) {
                            self.torrent.release_request(m.req, me);
                            break;
                        }
                        self.note_arrival_request(m.req, net.as_ref());
                        if self.state.outstanding_requests.is_empty() {
                            self.state.rtt.on_request_sent(m.req);
                        }
                        self.state.outstanding_requests.insert(m.req);
                        self.state.cancelled_requests.remove(&m.req);
                        self.cancelled_at.remove(&m.req);
                        *self.state.valid_receive_chunks.entry(m.req).or_default() += 1;
                        self.torrent.mark_chunk_requested(m.req);
                        self.torrent.record_rescue();
                    }
                }
            }
        }

        if tuning.hedge {
            let moves = hedge_decisions(
                &clock,
                tuning,
                &view,
                &peers,
                learned,
                &features_for,
                &learned_od,
            );
            let me = self.key();
            let plen = self.torrent.piece_length();
            let total = self.torrent.total_length();
            for m in moves {
                if m.hedger != me || self.state.peer_choking {
                    continue;
                }
                if self.state.outstanding_requests.contains(&m.req) {
                    continue;
                }
                if pending_requests(tuning, self.state.outstanding_requests.len() as u32,
                    self.state.cancelled_requests.len() as u32) >= self.nominal_max() as u32 { break; }
                let coord = block_coord_from_req_index(m.req, plen, total);
                if !(self.state.peer_sent_have_all
                    || self.state.peer_pieces.contains(&coord.piece)
                    || self.state.peer_allowed_fast.contains(&coord.piece))
                {
                    continue;
                }
                // Eager seek hedges are charged when sent: other peers
                // compute the same moves but only the hedger applies them.
                if m.eager && !self.torrent.take_seek_hedge() { continue; }
                if !self.torrent.claim_hedge(m.req, me) {
                    if m.eager { self.torrent.refund_seek_hedge(); }
                    continue;
                }
                if !handle.send_message(Message::Request {
                        index: coord.piece,
                        begin: coord.offset,
                        length: coord.length.max(1),
                    }) {
                    self.torrent.clear_hedge(m.req);
                    if m.eager { self.torrent.refund_seek_hedge(); }
                    break;
                }
                self.note_arrival_request(m.req, net.as_ref());
                self.state.outstanding_requests.insert(m.req);
                *self.state.valid_receive_chunks.entry(m.req).or_insert(0) += 1;
                self.torrent.mark_chunk_requested(m.req);
                self.torrent.record_hedge();
            }
        }
    }

    fn handle_choke(&mut self, handle: &ConnHandle) {
        wt_types::probe::add(wt_types::probe::CHOKES, 1);
        self.state.peer_choking = true;
        self.arrival.write().unchoked_at = None;
        self.state.webtor_choked_since = Some(Instant::now());
        // Cancel outstanding except allowed-fast
        let plen = self.torrent.piece_length();
        let total = self.torrent.total_length();
        let reqs: Vec<_> = self.state.outstanding_requests.iter().copied().collect();
        for r in reqs {
            let piece = block_coord_from_req_index(r, plen, total).piece;
            if self.state.peer_allowed_fast.contains(&piece) {
                continue;
            }
            self.cancel_outstanding(handle, r);
        }
        self.state.need_request_update = true;
        self.torrent.bump_change();
    }

    fn handle_unchoke(&mut self) {
        self.state.peer_choking = false;
        self.state.webtor_unchoked_at = Some(Instant::now());
        self.arrival.write().unchoked_at = self.state.webtor_unchoked_at;
        self.state.webtor_choked_since = None;
        self.state.need_request_update = true;
    }

    fn handle_have(&mut self, index: u32) {
        if self.state.peer_pieces.insert(index) {
            self.torrent.note_availability(index, 1);
            self.torrent.note_peer_piece(self.key(), index, true);
            // Go peerSentHave only schedules requests for new wanted pieces.
            // Announcements of completed/unselected pieces change availability,
            // but do not justify rebuilding every candidate block and queue.
            if self.torrent.piece_priority(index) > PiecePriority::None
                && !self.torrent.piece_complete(index)
            {
                self.state.need_request_update = true;
            }
        }
    }

    fn handle_bitfield(&mut self, handle: &ConnHandle, bf: &Bytes) {
        let n = self.torrent.num_pieces();
        // Back to counting this peer piece by piece, as the code below expects.
        if self.state.peer_sent_have_all {
            self.torrent.note_have_all_availability(-1);
            for &index in &self.state.peer_pieces {
                self.torrent.note_availability(index, 1);
            }
        }
        let next: HashSet<_> = unmarshal_bitfield(bf)
            .into_iter()
            .enumerate()
            .filter_map(|(i, has)| {
                let index = i as u32;
                (has && (n == 0 || index < n)).then_some(index)
            })
            .collect();
        let removed: HashSet<_> = self.state.peer_pieces.difference(&next).copied().collect();
        for &index in &removed {
            self.torrent.note_availability(index, -1);
            self.torrent.note_peer_piece(self.key(), index, false);
        }
        for &index in next.difference(&self.state.peer_pieces) {
            self.torrent.note_availability(index, 1);
            if self.torrent.piece_priority(index) > PiecePriority::None
                && !self.torrent.piece_complete(index)
            {
                self.state.need_request_update = true;
            }
        }
        let mut added = next.difference(&self.state.peer_pieces).copied();
        self.torrent.note_peer_pieces(self.key(), &mut added);
        self.state.peer_sent_have_all = false;
        self.state.peer_pieces = next;
        // A replacement bitfield / HaveNone retracts old availability. Do not
        // keep waiting for blocks the peer no longer claims it can serve.
        let unavailable: Vec<_> = self
            .state
            .outstanding_requests
            .iter()
            .copied()
            .filter(|&req| {
                removed.contains(
                    &block_coord_from_req_index(
                        req,
                        self.torrent.piece_length(),
                        self.torrent.total_length(),
                    )
                    .piece,
                )
            })
            .collect();
        for req in unavailable {
            self.cancel_outstanding(handle, req);
            self.state.need_request_update = true;
        }
    }

    fn handle_piece(
        &mut self,
        handle: &ConnHandle,
        index: u32,
        begin: u32,
        data: &[u8],
    ) -> Result<()> {
        let plen = self.torrent.piece_length();
        let req = req_index_from_piece_and_offset(index, begin, plen);
        let expected = self.state.outstanding_requests.contains(&req)
            || self
                .state
                .valid_receive_chunks
                .get(&req)
                .copied()
                .unwrap_or(0)
                > 0;
        if let Some((probe, sent)) = self.speed_probe {
            if probe == req {
                self.speed_probe = None;
                self.state.download_rate.seed(data.len(), sent.elapsed());
                if self.state.rtt.estimate.is_zero() {
                    self.state.rtt.estimate = sent.elapsed();
                }
                wt_types::events::record("speed_probe", format!("{}ms", sent.elapsed().as_millis()));
            }
        }
        if !expected {
            wt_types::probe::add(wt_types::probe::LATE, 1);
            return Ok(());
        }
        let me = self.key();
        // Other holders of this block, captured before receipt clears them.
        // Go cancels them synchronously; here they reconcile on their wake.
        let owner = self.torrent.requesting_peer(req);
        let hedge = self.torrent.hedge_peer(req);

        // Drop local outstanding before write so a failed write can be
        // re-requested (Go unpends before WriteAt, then pendRequest on error).
        self.state.outstanding_requests.remove(&req);
        self.state.cancelled_requests.remove(&req);
        self.cancelled_at.remove(&req);
        self.torrent.clear_chunk_requested(req);
        if hedge == Some(me) {
            self.torrent.clear_hedge(req);
        }
        if let Some(c) = self.state.valid_receive_chunks.get_mut(&req) {
            *c = c.saturating_sub(1);
        }
        if self.state.valid_receive_chunks.get(&req) == Some(&0) {
            self.state.valid_receive_chunks.remove(&req);
        }

        // Go learns arrival timing before its haveChunk check.
        self.note_arrival_block(req);

        // Go `haveChunk`: a late, cancelled or hedged copy of a block that is
        // written, or being written by another peer, is wasted — never
        // rewritten or counted as useful. The reservation is atomic, so two
        // peers delivering the same block together cannot both write it.
        if !self.torrent.begin_chunk_write(req) {
            wt_types::probe::add(wt_types::probe::WASTED, 1);
            self.state.stats.chunks_read_wasted.fetch_add(1, Ordering::Relaxed);
            handle.stats().chunks_read_wasted.fetch_add(1, Ordering::Relaxed);
            // Give up any claim so a failed write by the winner can be retried.
            self.torrent.release_request(req, me);
            self.torrent.release_hedge(req, me);
            if self.wants_refill() {
                self.state.need_request_update = true;
                handle.wake();
            }
            return Ok(());
        }

        // Almost every block lands in an existing RAM slot. Handing the
        // worker's core to another thread for that copy cost more than the
        // copy, on every 16 KiB block.
        let written = match self.torrent.write_chunk_in_memory(index, begin, data) {
            Some(written) => written,
            None => {
                wt_types::probe::add(wt_types::probe::SLOW_WRITE, 1);
                blocking_io(|| self.torrent.write_chunk(index, begin, data))
            }
        };
        // Marks the block dirty (readable) only after a successful write.
        self.torrent.finish_chunk_write(req, written.is_ok());
        if let Err(err) = written {
            // Go Peer.receiveChunk: log, pendRequest, onNeedUpdateRequests,
            // onWriteChunkErr; return nil — never kill the peer for a
            // transient storage/cache failure (e.g. stream cache busy).
            tracing::debug!("writing received chunk piece={index} begin={begin}: {err}");
            self.torrent.report_write_chunk_error(&err);
            self.torrent.release_request(req, me);
            self.state.need_request_update = true;
            // The block is requestable again: any peer may fetch it.
            self.torrent.bump_change();
            return Ok(());
        }

        wt_types::probe::add(wt_types::probe::RECV, 1);
        self.state.rtt.on_request_received(req);
        self.state.last_useful_chunk_received = Some(Instant::now());
        self.state.download_rate.add_bytes(data.len());
        self.state.stats.add_bytes_read_useful(data.len() as i64);
        // The I/O driver accounts wire bytes; the handler owns useful bytes.
        handle.stats().bytes_read_useful_data.fetch_add(data.len() as i64, Ordering::Relaxed);
        // First useful chunk forgives prior unhelpful dial penalties so a later
        // normal disconnect can redial without waiting out stale backoff.
        self.torrent.note_peer_helpful(self.state.remote_addr);
        self.torrent.on_chunk_received(me, req, data.len());
        // A displaced owner or hedge cancels its now-redundant request.
        for other in [owner, hedge].into_iter().flatten() {
            if other != me {
                self.torrent.wake_peer(other);
            }
        }

        if self.torrent.piece_all_dirty(index) {
            self.torrent.queue_piece_hash(index);
        }

        if self.wants_refill() {
            self.state.need_request_update = true;
            handle.wake();
        }
        Ok(())
    }

    fn handle_request(
        &mut self,
        handle: &ConnHandle,
        index: u32,
        begin: u32,
        length: u32,
    ) -> Result<()> {
        if self.state.am_choking && !self.state.peer_allowed_fast.contains(&index) {
            self.send(
                handle,
                Message::Reject {
                    index,
                    begin,
                    length,
                },
            );
            return Ok(());
        }
        let cfg = self.torrent.choke_config();
        let input = PeerChokeInput {
            id: self.key(),
            interested: self.state.peer_interested,
            currently_choking: self.state.am_choking,
            download_rate: self.state.download_rate.rate(),
            upload_rate: self.state.upload_rate.rate(),
            bytes_downloaded: self.state.stats.snapshot().bytes_read_useful_data,
            bytes_uploaded: self.state.stats.snapshot().bytes_written_data,
            is_snubbed: self.state.is_snubbed,
        };
        let wanted = true; // peer asked → they want something we have
        if !upload_allowed(&input, &cfg, wanted) {
            self.send(
                handle,
                Message::Reject {
                    index,
                    begin,
                    length,
                },
            );
            return Ok(());
        }
        // A new request supersedes an earlier cancel of the same block.
        handle.clear_peer_cancel(index, begin, length);
        // In RAM it is a copy. On disk it must not hold up this task, which also
        // receives this peer's blocks, nor a runtime worker. Peers may be served
        // out of order, so the block is sent when the read finishes.
        let data = match self.torrent.read_chunk_in_memory(index, begin, length) {
            Some(data) => {
                wt_types::probe::add(wt_types::probe::UP_MEM, 1);
                data?
            }
            None => match tokio::runtime::Handle::try_current() {
                Ok(rt) if rt.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                    wt_types::probe::add(wt_types::probe::UP_DISK, 1);
                    let torrent = Arc::clone(&self.torrent);
                    let handle = handle.clone();
                    rt.spawn_blocking(move || match torrent.read_chunk(index, begin, length) {
                        Ok(piece) => {
                            handle.send_message(Message::Piece { index, begin, piece });
                        }
                        Err(err) => handle.close(format!("handler error: {err}")),
                    });
                    self.state.upload_rate.add_bytes(length as usize);
                    self.state.stats.add_bytes_written(i64::from(length));
                    return Ok(());
                }
                _ => self.torrent.read_chunk(index, begin, length)?,
            },
        };
        let len = data.len();
        if !handle.send_message(Message::Piece {
            index,
            begin,
            piece: data,
        }) {
            return Ok(());
        }
        self.state.upload_rate.add_bytes(len);
        self.state.stats.add_bytes_written(len as i64);
        Ok(())
    }

    fn handle_extended(&mut self, handle: &ConnHandle, id: u8, payload: Bytes) -> Result<()> {
        if id == EXT_HANDSHAKE_ID {
            let ext =
                ExtendedHandshake::decode(&payload).map_err(|e| CoreError::Other(e.to_string()))?;
            if let Some(&id) = ext.m.get(EXTENSION_NAME_METADATA) {
                self.state.peer_ext_ut_metadata = (id != 0).then_some(id);
            }
            if let Some(&id) = ext.m.get(EXTENSION_NAME_PEX) {
                self.state.peer_ext_ut_pex = (id != 0).then_some(id);
            }
            self.state.peer_metadata_size = ext.metadata_size;
            if let Some(name) = ext.v.clone() {
                self.state.client_name = name;
            }
            if let Some(reqq) = ext.reqq {
                if reqq > 0 {
                    self.state.max_requests = (reqq as usize).clamp(1, MAX_LOCAL_TO_REMOTE_REQUESTS);
                }
            }
            if let Some(port) = ext.p {
                if port > 0 {
                    self.state.peer_listen_port = port;
                    self.torrent
                        .on_peer_listen_port(self.state.remote_addr, port);
                }
            }
            // Go: t.pex.Add(c); c.pex.Init(c) once the extended handshake arrives.
            if !self.torrent.pex_disabled() && !self.pex_listed {
                let utp = matches!(self.state.transport, Transport::Utp);
                self.torrent.pex_note_connected(
                    self.state.remote_addr,
                    self.state.encrypted,
                    utp,
                );
                self.pex_listed = true;
            }
            self.request_metadata_pieces(handle);
            self.state.need_request_update = true;
            self.update_requests(handle);
            // Go pex.Init AfterFunc(0) wakes the writer to Share immediately.
            self.maybe_send_pex(handle);
            return Ok(());
        }
        // BEP 10 mappings are directional: our IDs route inbound messages;
        // the remote IDs are used only when sending messages to that peer.
        if id == self.our_ext_metadata {
            // Malformed / non-metadata payloads on this id are common; do not
            // kill the connection (was "missing msg_type" mass disconnect).
            let msg = match MetadataMsg::decode(payload) {
                Ok(m) => m,
                Err(_) => return Ok(()),
            };
            match msg.msg_type {
                MetadataMsgType::Request => {
                    let Some(reply_id) = self.state.peer_ext_ut_metadata else {
                        return Ok(());
                    };
                    if let Some((total, data)) = self.torrent.metadata_piece(msg.piece) {
                        let resp = MetadataMsg::data(msg.piece, total, data);
                        self.send(handle, resp.to_message(reply_id));
                    } else {
                        let rej = MetadataMsg::reject(msg.piece);
                        self.send(handle, rej.to_message(reply_id));
                    }
                }
                MetadataMsgType::Data => {
                    self.peer_metadata.clear_requested(msg.piece);
                    if let Some(data) = msg.data {
                        let total = msg.total_size.unwrap_or(data.len() as u32);
                        let done = self.torrent.on_metadata_data(msg.piece, total, data)?;
                        if done {
                            self.state.need_request_update = true;
                            self.update_requests(handle);
                        } else {
                            self.request_metadata_pieces(handle);
                        }
                    }
                }
                MetadataMsgType::Reject => {
                    self.peer_metadata.clear_requested(msg.piece);
                    self.request_metadata_pieces(handle);
                }
            }
            return Ok(());
        }
        if id == self.our_ext_pex && !self.torrent.pex_disabled() {
            // Soft-discard malformed ut_pex like metadata; do not kill the conn.
            let msg = match PexMsg::decode(&payload) {
                Ok(m) => m,
                Err(_) => return Ok(()),
            };
            self.torrent.ingest_pex(self.state.remote_addr, &msg)?;
        }
        Ok(())
    }
}

impl<T: PeerTorrent + 'static> ConnHandler for PeerConnHandler<T> {
    fn on_wake(&mut self, handle: &ConnHandle) {
        // Go `fillWriteBuffer` → `upload`: choke/unchoke only via uploadAllowed.
        self.reconcile_choke(handle);
        if self.torrent.data_download_disallowed() {
            let requests: Vec<_> = self.state.outstanding_requests.iter().copied().collect();
            for req in requests {
                self.cancel_outstanding(handle, req);
            }
        } else {
            self.expire_stale_requests(handle);
            let displaced = !self.torrent
                .displaced_requests(&self.state.outstanding_requests, self.key())
                .is_empty();
            // Go NudgeUrgentRequests: only peers with spare request slots.
            let nudged = handle.take_update_request()
                && self.state.outstanding_requests.len() < self.nominal_max();
            // An empty choked queue cannot receive ordinary blocks. Rebuilding
            // the entire request snapshot on each 100 ms idle wake multiplied
            // scheduler CPU by every choked peer. Have/bitfield, allowed-fast,
            // unchoke and explicit request updates still run immediately.
            let can_request = !self.state.peer_choking || !self.state.peer_allowed_fast.is_empty();
            // Selection can become wanted after the peer's bitfield arrived.
            // A choked peer must still learn that interest changed so it can
            // unchoke us; this is a cheap piece query, not a request snapshot.
            let needs_interest = !self.state.am_interested
                && if self.state.peer_sent_have_all {
                    self.torrent.any_wanted_incomplete()
                } else {
                    self.torrent.any_wanted_among(&self.state.peer_pieces)
                };
            // Go only runs updateRequests when low on requests / needRequestUpdate /
            // refocus — NOT on every wake merely because request_focus is set.
            // The old `focus.is_some()` bypass refilled every 100ms under PeakWholeQueue
            // and grew pending to 300–500 (Bunny peerfix) while Go stayed ~20.
            if self.state.need_request_update
                || needs_interest
                || (can_request && self.wants_refill())
                || displaced
                || nudged
                || handle.refocus_pending()
            {
                self.update_requests(handle);
            }
        }
        self.maybe_probe_speed(handle);
        self.maybe_warm(handle);
        self.request_metadata_pieces(handle);
        self.maybe_send_pex(handle);
        self.update_expecting();
        if self.last_snapshot.elapsed() >= SNAPSHOT_EVERY {
            self.last_snapshot = Instant::now();
            self.torrent.update_peer_snapshot(self.peer_conn_info());
        }
    }

    fn wake_interval(&self) -> Option<Duration> {
        if !self.torrent.has_metadata() {
            return Some(Duration::from_secs(1));
        }
        // Reconcile choke when seed policy or tit-for-tat gate may have flipped.
        let cfg = self.torrent.choke_config();
        let input = PeerChokeInput {
            id: self.key(),
            interested: self.state.peer_interested,
            currently_choking: self.state.am_choking,
            download_rate: self.state.download_rate.rate(),
            upload_rate: self.state.upload_rate.rate(),
            bytes_downloaded: self.state.stats.snapshot().bytes_read_useful_data,
            bytes_uploaded: self.state.stats.snapshot().bytes_written_data,
            is_snubbed: self.state.is_snubbed,
        };
        let should_choke = !upload_allowed(&input, &cfg, self.peer_has_wanted_pieces());
        if self.state.am_choking != should_choke {
            return Some(Duration::from_millis(50));
        }
        // Nothing to expire or refill until the peer unchokes, and that message
        // refills at once. Selection changes wake us. Polling these at 100 ms
        // cost a global lock per choked peer, ten times a second.
        if self.state.peer_choking
            && self.state.peer_allowed_fast.is_empty()
            && self.state.outstanding_requests.is_empty()
            && self.state.cancelled_requests.is_empty()
            && !self.state.need_request_update
        {
            return Some(IDLE_CHOKED_WAKE);
        }
        // Refill stalled Normal-priority pipelines (download_all) and keep PEX alive.
        if !self.state.outstanding_requests.is_empty() || self.state.need_request_update {
            return Some(Duration::from_millis(100));
        }
        if self.torrent.any_wanted_incomplete() {
            return Some(Duration::from_millis(100));
        }
        if self.state.peer_ext_ut_pex.is_some() && !self.torrent.pex_disabled() {
            // First / empty snapshot retries at Go pexRetryDelay; steady state at pexInterval.
            if self.pex_cursor.is_none() {
                Some(PEX_RETRY_DELAY)
            } else {
                Some(PEX_INTERVAL)
            }
        } else {
            None
        }
    }

    fn on_message(&mut self, handle: &ConnHandle, msg: Message) -> Result<()> {
        let is_block = matches!(msg, Message::Piece { .. });
        match msg {
            Message::Keepalive => {}
            Message::Choke => self.handle_choke(handle),
            Message::Unchoke => {
                self.handle_unchoke();
                self.update_requests(handle);
                self.maybe_probe_speed(handle);
            }
            Message::Interested => {
                self.state.peer_interested = true;
                handle.wake();
            }
            Message::NotInterested => {
                self.state.peer_interested = false;
                handle.wake();
            }
            Message::Have { index } => {
                self.handle_have(index);
            }
            Message::Bitfield(bf) => {
                self.handle_bitfield(handle, &bf);
            }
            Message::Request {
                index,
                begin,
                length,
            } => self.handle_request(handle, index, begin, length)?,
            Message::Piece {
                index,
                begin,
                piece,
            } => {
                self.handle_piece(handle, index, begin, &piece)?;
            }
            Message::Cancel {
                index,
                begin,
                length,
            } => {
                // Go onPeerSentCancel: drop the block if not yet sent.
                handle.note_peer_cancel(index, begin, length);
            }
            Message::Port(port) => {
                self.state.peer_listen_port = port;
                self.torrent
                    .on_peer_listen_port(self.state.remote_addr, port);
                let addr = SocketAddr::new(self.state.remote_addr.ip(), port);
                self.torrent.add_dht_node(addr);
            }
            Message::Suggest { index } => {
                // Prefer requesting this piece next.
                let _ = index;
                self.state.need_request_update = true;
            }
            Message::HaveAll => {
                if !self.state.peer_sent_have_all {
                    let n = self.torrent.num_pieces();
                    for &piece in &self.state.peer_pieces {
                        self.torrent.note_availability(piece, -1);
                    }
                    self.torrent.note_have_all_availability(1);
                    self.state.peer_pieces.extend(0..n);
                    self.torrent.note_peer_pieces(self.key(), &mut (0..n));
                    self.state.peer_sent_have_all = true;
                    self.state.need_request_update = true;
                    self.torrent.bump_change();
                    self.update_requests(handle);
                }
            }
            Message::HaveNone => {
                self.handle_bitfield(handle, &Bytes::new());
            }
            Message::Reject {
                index,
                begin,
                length: _,
            } => {
                let plen = self.torrent.piece_length();
                let total = self.torrent.total_length();
                let req = req_index_from_piece_and_offset(index, begin, plen);
                let _ = total;
                // Go remoteRejectedRequest: rejecting a live request (not
                // acknowledging our cancel) shrinks the peer's peak.
                if self.state.outstanding_requests.remove(&req) {
                    self.state.peak_requests = self.state.peak_requests.saturating_sub(1);
                }
                // Go decExpectedChunkReceive.
                if let Some(c) = self.state.valid_receive_chunks.get_mut(&req) {
                    *c = c.saturating_sub(1);
                    if *c == 0 {
                        self.state.valid_receive_chunks.remove(&req);
                    }
                }
                self.state.cancelled_requests.remove(&req);
                self.cancelled_at.remove(&req);
                self.arrival.write().samples.remove(&req);
                if self.torrent.requesting_peer(req) == Some(self.key()) {
                    self.torrent.clear_chunk_requested(req);
                    self.torrent.clear_assignment(req);
                }
                if self.torrent.hedge_peer(req) == Some(self.key()) {
                    self.torrent.release_hedge(req, self.key());
                }
                self.state.need_request_update = true;
                self.update_requests(handle);
            }
            Message::AllowedFast { index } => {
                self.state.peer_allowed_fast.insert(index);
                self.state.need_request_update = true;
                self.update_requests(handle);
            }
            Message::Extended { id, payload } => {
                self.handle_extended(handle, id, payload)?;
            }
            Message::HashRequest { .. } => {
                // BEP 52: reject by default until v2 piece layers are wired.
            }
            Message::Hashes { .. } => {}
            Message::HashReject { .. } => {}
        }
        self.after_message(handle, is_block);
        Ok(())
    }

    fn on_piece(&mut self, handle: &ConnHandle, index: u32, begin: u32, data: &[u8]) -> Result<()> {
        self.handle_piece(handle, index, begin, data)?;
        self.after_message(handle, true);
        Ok(())
    }

    fn on_closed(&mut self, _handle: &ConnHandle, _reason: &str) {
        wt_types::probe::add(wt_types::probe::CONN_CLOSED, 1);
        self.closed.store(true, Ordering::Relaxed);
        if self.pex_listed {
            let utp = matches!(self.state.transport, Transport::Utp);
            self.torrent
                .pex_note_closed(self.state.remote_addr, self.state.encrypted, utp);
            self.pex_listed = false;
        }
        // Drop availability
        if self.state.peer_sent_have_all {
            self.torrent.note_have_all_availability(-1);
        } else {
            for &p in &self.state.peer_pieces {
                self.torrent.note_availability(p, -1);
            }
        }
        let reqs: Vec<_> = self.state.outstanding_requests.iter().copied().collect();
        for r in reqs {
            self.torrent.clear_chunk_requested(r);
            if self.torrent.requesting_peer(r) == Some(self.key()) {
                self.torrent.clear_assignment(r);
            }
        }
        self.torrent.peer_closed(self.state.remote_addr);
        self.torrent.bump_change();
    }
}

// AtomicConnStats helpers used above — add thin wrappers if missing.
trait ConnStatsExt {
    fn add_bytes_read_useful(&self, n: i64);
    fn add_bytes_written(&self, n: i64);
}

impl ConnStatsExt for AtomicConnStats {
    fn add_bytes_read_useful(&self, n: i64) {
        // Keep bytes_read_data in sync — /connections reports that field.
        self.bytes_read_data.fetch_add(n, Ordering::Relaxed);
        self.bytes_read_useful_data.fetch_add(n, Ordering::Relaxed);
    }
    fn add_bytes_written(&self, n: i64) {
        self.bytes_written_data.fetch_add(n, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- Webtor view adapters

struct PeerTorrentView<'a, T: PeerTorrent>(&'a T);

impl<T: PeerTorrent> TorrentView for PeerTorrentView<'_, T> {
    type PeerId = PeerKey;

    fn request_focus_index(&self) -> Option<RequestIndex> {
        self.0.request_focus_index()
    }
    fn eager_hedge(&self) -> bool {
        self.0.seek_hedge_open()
    }
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority {
        self.0.piece_priority(piece)
    }
    fn piece_of_request(&self, req: RequestIndex) -> PieceIndex {
        block_coord_from_req_index(req, self.0.piece_length(), self.0.total_length()).piece
    }
    fn block_arrived(&self, req: RequestIndex) -> bool {
        self.0.chunk_dirty(req) || self.0.piece_complete(self.piece_of_request(req))
    }
    fn hashing_or_marking(&self, piece: PieceIndex) -> bool {
        self.0.piece_hashing(piece)
    }
    fn max_end_request(&self) -> RequestIndex {
        let chunk = i64::from(self.0.chunk_size().max(1));
        ((self.0.total_length() + chunk - 1) / chunk) as RequestIndex
    }
    fn request_when(&self, req: RequestIndex) -> Option<Instant> {
        self.0.request_when(req)
    }
    fn requesting_peer(&self, req: RequestIndex) -> Option<Self::PeerId> {
        self.0.requesting_peer(req)
    }
    fn piece_bytes_left(&self, piece: PieceIndex) -> u64 {
        if self.0.piece_complete(piece) {
            return 0;
        }
        let plen = self.0.piece_length();
        let total = self.0.total_length();
        let (start, end) = piece_req_index_range(piece, plen, total);
        let mut left = 0u64;
        for r in start..end {
            if !self.0.chunk_dirty(r) {
                left += u64::from(block_coord_from_req_index(r, plen, total).length);
            }
        }
        left
    }
    fn hedge_peer(&self, req: RequestIndex) -> Option<Self::PeerId> {
        self.0.hedge_peer(req)
    }
    fn chunk_size(&self) -> u32 {
        self.0.chunk_size()
    }
    fn piece_complete(&self, piece: PieceIndex) -> bool {
        self.0.piece_complete(piece)
    }
    fn num_pieces(&self) -> PieceIndex {
        self.0.num_pieces()
    }
}

struct HolPeerSnap<T: PeerTorrent> {
    key: PeerKey,
    download_rate_bps: f64,
    rtt: Duration,
    queued: u32,
    cancelled: u32,
    nominal_max: u32,
    peer_choking: bool,
    closed: bool,
    last_useful: Option<Instant>,
    torrent: Arc<T>,
    valid_receive: HashMap<RequestIndex, u32>,
    pieces: HashSet<u32>,
    have_all: bool,
    lookup_torrent: bool,
    arrival_inputs: Option<[f64; ARRIVAL_FEATURES]>,
    arrival: Option<Arc<RwLock<PeerArrivalState>>>,
}

impl<T: PeerTorrent> PeerView for HolPeerSnap<T> {
    type Id = PeerKey;

    fn id(&self) -> Self::Id {
        self.key
    }
    fn download_rate_bps(&self) -> f64 {
        self.download_rate_bps
    }
    fn rtt(&self) -> Duration {
        self.rtt
    }
    fn queued_requests(&self) -> u32 {
        self.queued
    }
    fn cancelled_requests(&self) -> u32 {
        self.cancelled
    }
    fn nominal_max_requests(&self) -> u32 {
        self.nominal_max
    }
    fn last_useful_data(&self) -> Option<Instant> {
        self.last_useful
    }
    fn has_piece(&self, piece: PieceIndex) -> bool {
        if self.have_all {
            true
        } else if self.lookup_torrent {
            self.torrent.peer_has_piece(self.key, piece)
        } else {
            self.pieces.contains(&piece)
        }
    }
    fn peer_choking(&self) -> bool {
        self.peer_choking
    }
    fn closed(&self) -> bool {
        self.closed
    }
    fn valid_receive_count(&self, req: RequestIndex) -> u32 {
        self.valid_receive.get(&req).copied().unwrap_or(0)
    }
}

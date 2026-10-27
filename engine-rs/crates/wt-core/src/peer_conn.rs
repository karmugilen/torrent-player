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
use wt_wire::message::{unmarshal_bitfield, Message};

use crate::arrival::{ArrivalPeer, ArrivalSample, ARRIVAL_FEATURES, ARRIVAL_UNCHOKE_CAP};
use crate::choking::{upload_allowed, ChokeConfig, PeerChokeInput};
use crate::conn::{ConnHandle, ConnHandler};
use crate::picker::{block_coord_from_req_index, req_index_from_piece_and_offset};
use crate::stats::{AtomicConnStats, PeerConnInfo, RateEstimator};
use crate::strategy::{
    compute_desired_requests, sort_requests_for_peer, DesiredPeerSnapshot, PieceRequestOrder,
    RequestTorrentSnapshot, RequestTuning,
};
use crate::tuning::{current_seek_tuning, SeekTuning};
use crate::webtor::{
    expected_arrival_secs, hedge_answered, hedge_decisions, may_take_existing_request,
    next_peak_requests, pending_requests, rescue_decisions, PeerView, SystemClock, TorrentView,
    SLOW_PEER_BYTES_PER_SEC,
};
use crate::wire::{DEFAULT_UT_METADATA_ID, DEFAULT_UT_PEX_ID, EXT_HANDSHAKE_ID};
use crate::{CoreError, Result};

// ---------------------------------------------------------------- PeerTorrent

/// Opaque peer identity in torrent assignment maps.
pub type PeerKey = u64;

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
pub trait PeerTorrent: Send + Sync {
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
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority;
    fn chunk_dirty(&self, req: RequestIndex) -> bool;
    fn mark_chunk_dirty(&self, req: RequestIndex);
    fn clear_chunk_requested(&self, req: RequestIndex);
    fn mark_chunk_requested(&self, req: RequestIndex);
    fn piece_all_dirty(&self, piece: PieceIndex) -> bool;
    fn note_availability(&self, piece: PieceIndex, delta: i32);
    fn note_peer_piece(&self, _peer: PeerKey, _piece: PieceIndex, _has: bool) {}
    fn peer_closed(&self, _addr: SocketAddr) {}
    fn update_peer_snapshot(&self, _info: PeerConnInfo) {}

    fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()>;
    fn read_chunk(&self, piece: PieceIndex, begin: u32, length: u32) -> Result<Bytes>;
    fn queue_piece_hash(&self, piece: PieceIndex);

    fn requesting_peer(&self, req: RequestIndex) -> Option<PeerKey>;
    fn request_when(&self, req: RequestIndex) -> Option<Instant>;
    fn assign_request(&self, req: RequestIndex, peer: PeerKey, when: Instant);
    fn clear_assignment(&self, req: RequestIndex);
    fn desired_request_context(&self) -> (RequestTorrentSnapshot, PieceRequestOrder);

    fn request_focus_index(&self) -> Option<RequestIndex>;
    fn urgent_steal_after(&self) -> Duration;
    fn seek_tuning(&self) -> SeekTuning {
        current_seek_tuning()
    }
    fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey>;
    fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey);
    fn clear_hedge(&self, req: RequestIndex);
    fn record_rescue(&self) {}
    fn record_hedge(&self) {}
    fn peer_has_piece(&self, peer: PeerKey, piece: PieceIndex) -> bool;
    fn webtor_peer_snap(&self, peer: PeerKey) -> Option<WebtorPeerSnap>;
    fn connected_peer_keys(&self) -> Vec<PeerKey>;

    fn on_metadata_data(&self, piece: u32, total_size: u32, data: Bytes) -> Result<bool>;
    fn metadata_piece(&self, piece: u32) -> Option<(u32, Bytes)>;
    fn metadata_size(&self) -> Option<u32>;
    fn has_metadata(&self) -> bool;
    fn ingest_pex(&self, from: SocketAddr, msg: &PexMsg) -> Result<()>;
    fn add_dht_node(&self, addr: SocketAddr);
    fn on_peer_listen_port(&self, peer_addr: SocketAddr, port: u16);

    fn choke_config(&self) -> ChokeConfig;
    fn bump_change(&self);
    fn on_chunk_received(&self, peer: PeerKey, req: RequestIndex, len: usize);
}

// ---------------------------------------------------------------- RTT / legacy PeerConn

pub struct RttEstimator {
    pub probe: Option<RequestIndex>,
    pub probe_at: Option<Instant>,
    pub estimate: Duration,
}

impl Default for RttEstimator {
    fn default() -> Self {
        Self {
            probe: None,
            probe_at: None,
            estimate: Duration::from_millis(200),
        }
    }
}

impl RttEstimator {
    pub fn on_request_sent(&mut self, req: RequestIndex) {
        if self.probe_at.is_none()
            || self
                .probe_at
                .map(|t| t.elapsed() > Duration::from_secs(10))
                .unwrap_or(false)
        {
            self.probe = Some(req);
            self.probe_at = Some(Instant::now());
        }
    }

    pub fn on_request_received(&mut self, req: RequestIndex) {
        if let (Some(probe), Some(sent_at)) = (self.probe, self.probe_at) {
            if req == probe {
                let sample = sent_at.elapsed();
                self.probe = None;
                self.probe_at = None;
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

    pub max_requests: usize,
    pub peak_requests: usize,
    pub outstanding_requests: HashSet<RequestIndex>,
    pub cancelled_requests: HashSet<RequestIndex>,

    pub rtt: RttEstimator,
    pub download_rate: RateEstimator,
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
                max_requests: 64,
                peak_requests: 64,
                outstanding_requests: HashSet::new(),
                cancelled_requests: HashSet::new(),
                rtt: RttEstimator::default(),
                download_rate: RateEstimator::new(),
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
        let rate = inner.download_rate.rate();
        if rate <= 0.0 {
            return None;
        }
        let rtt = inner.rtt.estimate.as_secs_f64().max(0.001);
        let block = (chunk_size as f64) / rate;
        let recent = if inner.arrival.recent > 0.0 {
            inner.arrival.recent
        } else {
            rate
        };
        let idle = inner
            .arrival
            .last_block
            .map(|t| t.elapsed().as_secs_f64().min(10.0))
            .unwrap_or(10.0);
        let unchoked = inner
            .webtor_unchoked_at
            .map(|t| t.elapsed().as_secs_f64().min(ARRIVAL_UNCHOKE_CAP))
            .unwrap_or(ARRIVAL_UNCHOKE_CAP);

        Some([
            1.0,
            rtt,
            (queued as f64) * block,
            block,
            inner.arrival.jitter,
            (queued as f64) * (chunk_size as f64) / recent,
            idle,
            unchoked,
        ])
    }

    pub fn note_request_sent(
        &self,
        req: RequestIndex,
        chunk_size: usize,
        arrival_net: Option<&crate::arrival::ArrivalNet>,
    ) {
        let mut inner = self.inner.write();
        inner.outstanding_requests.insert(req);
        inner.cancelled_requests.remove(&req);
        inner.rtt.on_request_sent(req);

        if let Some(net) = arrival_net {
            if let Some(x) =
                self.arrival_inputs_locked(&inner, inner.outstanding_requests.len(), chunk_size)
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
        inner.last_useful_chunk_received = Some(Instant::now());

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
        let inner = self.inner.read();
        if use_learned {
            if let Some(sample) = inner.arrival_samples.get(&req) {
                let late = inner.arrival.overdue_duration(sample.estimate, min_timeout);
                return sample.sent.elapsed() >= late;
            }
        }
        false
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
            rtt: inner.rtt.estimate,
            queued: inner.outstanding_requests.len(),
            cancelled: inner.cancelled_requests.len(),
            connected_for: inner.connected_at.elapsed(),
            last_useful_data: inner.last_useful_chunk_received.map(|t| t.elapsed()),
        }
    }
}

// ---------------------------------------------------------------- ConnHandler

/// ConnHandler-backed peer state machine.
pub struct PeerConnHandler<T: PeerTorrent> {
    torrent: Arc<T>,
    state: PeerConnInner,
    closed: Arc<AtomicBool>,
    our_ext_metadata: u8,
    our_ext_pex: u8,
    metadata_requested: HashSet<u32>,
}

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
                source: PeerSource::Direct,
                encrypted: handle.is_encrypted(),
                connected_at: now,
                peer_choking: true,
                am_choking: true,
                peer_interested: false,
                am_interested: false,
                peer_pieces: HashSet::new(),
                peer_sent_have_all: false,
                peer_allowed_fast: HashSet::new(),
                max_requests: 64,
                peak_requests: 16,
                outstanding_requests: HashSet::new(),
                cancelled_requests: HashSet::new(),
                rtt: RttEstimator::default(),
                download_rate: RateEstimator::new(),
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
            metadata_requested: HashSet::new(),
        }
    }

    pub fn key(&self) -> PeerKey {
        self.torrent.peer_key(self.state.remote_addr)
    }

    pub fn start(&mut self, handle: &ConnHandle) {
        let mut ext = ExtendedHandshake::default();
        ext.m
            .insert(EXTENSION_NAME_METADATA.to_string(), self.our_ext_metadata);
        ext.m
            .insert(EXTENSION_NAME_PEX.to_string(), self.our_ext_pex);
        ext.metadata_size = self.torrent.metadata_size();
        self.send(
            handle,
            Message::Extended {
                id: EXT_HANDSHAKE_ID,
                payload: ext.encode(),
            },
        );
        let count = self.torrent.num_pieces();
        if count > 0 {
            let mut bits = vec![0u8; (count as usize).div_ceil(8)];
            for piece in 0..count {
                if self.torrent.piece_complete(piece) {
                    bits[piece as usize / 8] |= 0x80 >> (piece % 8);
                }
            }
            self.send(handle, Message::Bitfield(Bytes::from(bits)));
        }
        if !self.torrent.data_upload_disallowed() {
            self.state.am_choking = false;
            self.send(handle, Message::Unchoke);
        }
        self.update_requests(handle);
        self.torrent.update_peer_snapshot(self.peer_conn_info());
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
        let tuning = self.torrent.seek_tuning();
        if tuning.hol_rescue || tuning.hedge {
            self.apply_hol_and_hedge(handle, &tuning);
        }
        let req_tuning = RequestTuning::from_seek_tuning(
            tuning.bound_candidates,
            tuning.pipeline_seconds,
            tuning.min_pipeline,
            tuning.pipeline_rtt,
            self.torrent.request_focus_index().is_some(),
        );

        let (snap, order) = self.torrent.desired_request_context();
        let num_pieces = self.torrent.num_pieces();
        if self.state.peer_sent_have_all
            && num_pieces > 0
            && self.state.peer_pieces.len() < num_pieces as usize
        {
            for i in 0..num_pieces {
                if self.state.peer_pieces.insert(i) {
                    self.torrent.note_availability(i, 1);
                    self.torrent.note_peer_piece(self.key(), i, true);
                }
            }
        }
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
            outstanding: self.state.outstanding_requests.iter().copied().collect(),
            cancelled: self.state.cancelled_requests.iter().copied().collect(),
            nominal_max_requests: self.nominal_max() as u32,
        };

        let mut desired = compute_desired_requests(&snap, &peer_snap, &order, &req_tuning);
        sort_requests_for_peer(
            &mut desired.request_indexes,
            &snap,
            &peer_snap,
            &desired.piece_states,
        );

        // Interest
        if desired.interested != self.state.am_interested {
            self.state.am_interested = desired.interested;
            if desired.interested {
                self.state.last_became_interested = Some(Instant::now());
                self.send(handle, Message::Interested);
            } else {
                self.send(handle, Message::NotInterested);
            }
        }

        let was_empty = self.state.outstanding_requests.is_empty();
        let before = self.state.outstanding_requests.len() as u32;
        let clock = SystemClock;
        let steal_after = self.torrent.urgent_steal_after();
        let focus = self.torrent.request_focus_index();
        let focus_set = focus.is_some();
        let chunk = self.torrent.chunk_size();
        let plen = self.torrent.piece_length();
        let total = self.torrent.total_length();
        let mut added = 0u32;
        let mut yields_left = 10u32;

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

            // SlowPeerYield
            if tuning.slow_peer_yield
                && yields_left > 0
                && prio >= PiecePriority::Next
                && self.state.download_rate.rate() < SLOW_PEER_BYTES_PER_SEC
            {
                // Skip this piece's blocks once per refill budget.
                yields_left = yields_left.saturating_sub(1);
                continue;
            }

            if let Some(holder) = self.torrent.requesting_peer(r) {
                if holder != self.key() {
                    let holder_when = self.torrent.request_when(r).unwrap_or_else(Instant::now);
                    let holder_snap = self.torrent.webtor_peer_snap(holder);
                    let holder_uncancelled = holder_snap.as_ref().map(|s| s.queued).unwrap_or(1);
                    let holder_useful = holder_snap.and_then(|s| s.last_useful);
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
                    // Steal: clear assignment; holder cancels on its own refill.
                    self.torrent.clear_assignment(r);
                }
            }

            // fasterFocusPeer: leave playhead block to a sooner peer
            if steal_after > Duration::ZERO
                && tuning.faster_ratio > 0.0
                && focus.is_some_and(|f| r >= f && r < f + tuning.focus_blocks as u32)
                && prio >= PiecePriority::Now
            {
                let self_at = expected_arrival_secs(
                    self.state.download_rate.rate(),
                    self.state.rtt.estimate,
                    self.state.outstanding_requests.len() as u32,
                    chunk,
                );
                let ratio = if tuning.arrival_aware {
                    tuning.arrival_ratio.max(1.0)
                } else {
                    tuning.faster_ratio
                };
                let threshold = if tuning.arrival_aware {
                    self_at / ratio
                } else {
                    self_at // rate path handled roughly
                };
                let mut better = false;
                for pk in self.torrent.connected_peer_keys() {
                    if pk == self.key() || !self.torrent.peer_has_piece(pk, piece) {
                        continue;
                    }
                    if let Some(s) = self.torrent.webtor_peer_snap(pk) {
                        if s.closed || s.peer_choking {
                            continue;
                        }
                        let at = expected_arrival_secs(s.download_rate_bps, s.rtt, s.queued, chunk);
                        if tuning.arrival_aware {
                            if at < threshold {
                                better = true;
                                break;
                            }
                        } else if s.download_rate_bps > self.state.download_rate.rate() * ratio {
                            better = true;
                            break;
                        }
                    }
                }
                if better {
                    continue;
                }
            }

            if self.state.peer_choking && !self.state.peer_allowed_fast.contains(&piece) {
                continue;
            }

            let coord = block_coord_from_req_index(r, plen, total);
            self.send(
                handle,
                Message::Request {
                    index: coord.piece,
                    begin: coord.offset,
                    length: coord.length.max(1),
                },
            );
            self.state.outstanding_requests.insert(r);
            self.state.cancelled_requests.remove(&r);
            self.state.rtt.on_request_sent(r);
            self.torrent.mark_chunk_requested(r);
            self.torrent.assign_request(r, self.key(), Instant::now());
            added += 1;
        }

        let after = self.state.outstanding_requests.len() as u32;
        let peak = next_peak_requests(
            tuning.peak_mode,
            self.state.peak_requests as u32,
            after,
            added,
            was_empty,
        );
        self.state.peak_requests = peak as usize;
        let _ = before;
    }

    fn nominal_max(&self) -> usize {
        let tuning = self.torrent.seek_tuning();
        let base = (self.state.peak_requests * 2).max(1);
        if let Some(pipe) = self.state.webtor_pipeline_limit.checked_mul(1) {
            if pipe > 0 && self.torrent.request_focus_index().is_some() {
                return base.min(pipe).max(1);
            }
        }
        let _ = tuning;
        base.min(self.state.max_requests).max(1)
    }

    /// Apply HOL rescue / hedge moves that involve this peer (Go nudge path).
    fn apply_hol_and_hedge(&mut self, handle: &ConnHandle, tuning: &SeekTuning) {
        let clock = SystemClock;
        let view = PeerTorrentView(&*self.torrent);
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
            pieces: self.state.peer_pieces.clone(),
            have_all: self.state.peer_sent_have_all,
            lookup_torrent: false,
        });
        for pk in self.torrent.connected_peer_keys() {
            if pk == self.key() {
                continue;
            }
            if let Some(s) = self.torrent.webtor_peer_snap(pk) {
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
                });
            }
        }
        let features_for = |_p: &HolPeerSnap<T>| -> Option<[f64; ARRIVAL_FEATURES]> { None };
        let learned_od = |_p: &HolPeerSnap<T>, _r: RequestIndex| -> Option<bool> { None };

        if tuning.hol_rescue {
            let moves = rescue_decisions(&clock, tuning, &view, &peers, None, &features_for);
            let me = self.key();
            let plen = self.torrent.piece_length();
            let total = self.torrent.total_length();
            let chunk = self.torrent.chunk_size();
            for m in moves {
                if m.from == me && self.state.outstanding_requests.contains(&m.req) {
                    let coord = block_coord_from_req_index(m.req, plen, total);
                    self.send(
                        handle,
                        Message::Cancel {
                            index: coord.piece,
                            begin: coord.offset,
                            length: coord.length.max(chunk),
                        },
                    );
                    self.state.outstanding_requests.remove(&m.req);
                    self.state.cancelled_requests.insert(m.req);
                    self.torrent.clear_chunk_requested(m.req);
                    self.torrent.clear_assignment(m.req);
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
                        self.torrent.clear_assignment(m.req);
                        self.send(
                            handle,
                            Message::Request {
                                index: coord.piece,
                                begin: coord.offset,
                                length: coord.length.max(1),
                            },
                        );
                        self.state.outstanding_requests.insert(m.req);
                        self.torrent.mark_chunk_requested(m.req);
                        self.torrent.assign_request(m.req, me, Instant::now());
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
                None,
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
                let coord = block_coord_from_req_index(m.req, plen, total);
                if !(self.state.peer_sent_have_all
                    || self.state.peer_pieces.contains(&coord.piece)
                    || self.state.peer_allowed_fast.contains(&coord.piece))
                {
                    continue;
                }
                self.send(
                    handle,
                    Message::Request {
                        index: coord.piece,
                        begin: coord.offset,
                        length: coord.length.max(1),
                    },
                );
                self.state.outstanding_requests.insert(m.req);
                *self.state.valid_receive_chunks.entry(m.req).or_insert(0) += 1;
                self.torrent.set_hedge_peer(m.req, me);
                self.torrent.mark_chunk_requested(m.req);
                self.torrent.record_hedge();
            }
        }
    }

    fn handle_choke(&mut self, handle: &ConnHandle) {
        self.state.peer_choking = true;
        self.state.webtor_choked_since = Some(Instant::now());
        // Cancel outstanding except allowed-fast
        let plen = self.torrent.piece_length();
        let total = self.torrent.total_length();
        let chunk = self.torrent.chunk_size();
        let reqs: Vec<_> = self.state.outstanding_requests.iter().copied().collect();
        for r in reqs {
            let piece = block_coord_from_req_index(r, plen, total).piece;
            if self.state.peer_allowed_fast.contains(&piece) {
                continue;
            }
            let coord = block_coord_from_req_index(r, plen, total);
            self.send(
                handle,
                Message::Cancel {
                    index: coord.piece,
                    begin: coord.offset,
                    length: coord.length.max(chunk),
                },
            );
            self.state.outstanding_requests.remove(&r);
            self.state.cancelled_requests.insert(r);
            self.torrent.clear_chunk_requested(r);
            if self.torrent.requesting_peer(r) == Some(self.key()) {
                self.torrent.clear_assignment(r);
            }
        }
        self.state.need_request_update = true;
        self.torrent.bump_change();
    }

    fn handle_unchoke(&mut self) {
        self.state.peer_choking = false;
        self.state.webtor_unchoked_at = Some(Instant::now());
        self.state.webtor_choked_since = None;
        self.state.need_request_update = true;
        self.torrent.bump_change();
    }

    fn handle_have(&mut self, index: u32) {
        if self.state.peer_pieces.insert(index) {
            self.torrent.note_availability(index, 1);
            self.torrent.note_peer_piece(self.key(), index, true);
            self.state.need_request_update = true;
            self.torrent.bump_change();
        }
    }

    fn handle_bitfield(&mut self, bf: &Bytes) {
        let bits = unmarshal_bitfield(bf);
        let n = self.torrent.num_pieces();
        for (i, has) in bits.into_iter().enumerate() {
            let idx = i as u32;
            if n > 0 && idx >= n {
                break;
            }
            if has && self.state.peer_pieces.insert(idx) {
                self.torrent.note_availability(idx, 1);
                self.torrent.note_peer_piece(self.key(), idx, true);
            }
        }
        self.state.need_request_update = true;
        self.torrent.bump_change();
    }

    fn handle_piece(
        &mut self,
        handle: &ConnHandle,
        index: u32,
        begin: u32,
        data: Bytes,
    ) -> Result<()> {
        let plen = self.torrent.piece_length();
        let total = self.torrent.total_length();
        let req = req_index_from_piece_and_offset(index, begin, plen);
        let _ = total;
        let expected = self.state.outstanding_requests.contains(&req)
            || self
                .state
                .valid_receive_chunks
                .get(&req)
                .copied()
                .unwrap_or(0)
                > 0;
        if !expected {
            return Ok(());
        }

        self.torrent.write_chunk(index, begin, &data)?;
        self.torrent.mark_chunk_dirty(req);
        self.torrent.clear_chunk_requested(req);
        self.state.outstanding_requests.remove(&req);
        self.state.cancelled_requests.remove(&req);
        self.state.rtt.on_request_received(req);
        self.state.last_useful_chunk_received = Some(Instant::now());
        self.state.download_rate.add_bytes(data.len());
        self.state.stats.add_bytes_read_useful(data.len() as i64);
        handle.stats().add_bytes_read_useful(data.len() as i64);
        self.torrent.on_chunk_received(self.key(), req, data.len());

        {
            let mut hedges = HashMap::new();
            if let Some(h) = self.torrent.hedge_peer(req) {
                hedges.insert(req, h);
            }
            if hedge_answered(&hedges, req, self.key()) {
                self.torrent.clear_hedge(req);
            }
        }
        if let Some(c) = self.state.valid_receive_chunks.get_mut(&req) {
            *c = c.saturating_sub(1);
        }

        if self.torrent.piece_all_dirty(index) {
            self.torrent.queue_piece_hash(index);
        }

        self.state.need_request_update = true;
        self.torrent.bump_change();
        let _ = handle;
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
        let data = self.torrent.read_chunk(index, begin, length)?;
        self.state.upload_rate.add_bytes(data.len());
        self.state.stats.add_bytes_written(data.len() as i64);
        handle.stats().add_bytes_written(data.len() as i64);
        self.send(
            handle,
            Message::Piece {
                index,
                begin,
                piece: data,
            },
        );
        Ok(())
    }

    fn handle_extended(&mut self, handle: &ConnHandle, id: u8, payload: Bytes) -> Result<()> {
        if id == EXT_HANDSHAKE_ID {
            let ext =
                ExtendedHandshake::decode(&payload).map_err(|e| CoreError::Other(e.to_string()))?;
            self.state.peer_ext_ut_metadata = ext.m.get(EXTENSION_NAME_METADATA).copied();
            self.state.peer_ext_ut_pex = ext.m.get(EXTENSION_NAME_PEX).copied();
            self.state.peer_metadata_size = ext.metadata_size;
            if let Some(name) = ext.v.clone() {
                self.state.client_name = name;
            }
            // Request metadata if we lack it
            if !self.torrent.has_metadata() {
                if let Some(m_id) = self.state.peer_ext_ut_metadata {
                    let size = self.state.peer_metadata_size.unwrap_or(0);
                    if size > 0 {
                        let count = (size as usize).div_ceil(16 * 1024);
                        for piece in 0..count as u32 {
                            if self.metadata_requested.insert(piece) {
                                let req = MetadataMsg::request(piece);
                                self.send(handle, req.to_message(m_id));
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        if Some(id) == self.state.peer_ext_ut_metadata || id == self.our_ext_metadata {
            let msg = MetadataMsg::decode(payload).map_err(|e| CoreError::Other(e.to_string()))?;
            match msg.msg_type {
                MetadataMsgType::Request => {
                    if let Some((total, data)) = self.torrent.metadata_piece(msg.piece) {
                        let resp = MetadataMsg::data(msg.piece, total, data);
                        self.send(handle, resp.to_message(id));
                    } else {
                        let rej = MetadataMsg::reject(msg.piece);
                        self.send(handle, rej.to_message(id));
                    }
                }
                MetadataMsgType::Data => {
                    if let Some(data) = msg.data {
                        let total = msg.total_size.unwrap_or(data.len() as u32);
                        let done = self.torrent.on_metadata_data(msg.piece, total, data)?;
                        if done {
                            self.state.need_request_update = true;
                            self.update_requests(handle);
                        }
                    }
                }
                MetadataMsgType::Reject => {
                    self.metadata_requested.remove(&msg.piece);
                }
            }
            return Ok(());
        }
        if Some(id) == self.state.peer_ext_ut_pex || id == self.our_ext_pex {
            let msg = PexMsg::decode(&payload).map_err(|e| CoreError::Other(e.to_string()))?;
            self.torrent.ingest_pex(self.state.remote_addr, &msg)?;
        }
        Ok(())
    }
}

impl<T: PeerTorrent + 'static> ConnHandler for PeerConnHandler<T> {
    fn on_wake(&mut self, handle: &ConnHandle) {
        let should_choke = self.torrent.data_upload_disallowed();
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
        if self.torrent.data_download_disallowed() {
            let requests: Vec<_> = self.state.outstanding_requests.drain().collect();
            for req in requests {
                let coord = block_coord_from_req_index(
                    req,
                    self.torrent.piece_length(),
                    self.torrent.total_length(),
                );
                self.send(
                    handle,
                    Message::Cancel {
                        index: coord.piece,
                        begin: coord.offset,
                        length: coord.length,
                    },
                );
                self.torrent.clear_chunk_requested(req);
                if self.torrent.requesting_peer(req) == Some(self.key()) {
                    self.torrent.clear_assignment(req);
                }
            }
        } else {
            self.update_requests(handle);
        }
        self.torrent.update_peer_snapshot(self.peer_conn_info());
    }

    fn wake_interval(&self) -> Option<Duration> {
        if (0..self.torrent.num_pieces()).any(|i| {
            self.torrent.piece_priority(i) >= PiecePriority::Next && !self.torrent.piece_complete(i)
        }) {
            Some(Duration::from_millis(100))
        } else {
            None
        }
    }

    fn on_message(&mut self, handle: &ConnHandle, msg: Message) -> Result<()> {
        match msg {
            Message::Keepalive => {}
            Message::Choke => self.handle_choke(handle),
            Message::Unchoke => {
                self.handle_unchoke();
                self.update_requests(handle);
            }
            Message::Interested => {
                self.state.peer_interested = true;
                self.torrent.bump_change();
            }
            Message::NotInterested => {
                self.state.peer_interested = false;
                self.torrent.bump_change();
            }
            Message::Have { index } => {
                self.handle_have(index);
                self.update_requests(handle);
            }
            Message::Bitfield(bf) => {
                self.handle_bitfield(&bf);
                self.update_requests(handle);
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
                self.handle_piece(handle, index, begin, piece)?;
                self.update_requests(handle);
            }
            Message::Cancel {
                index,
                begin,
                length: _,
            } => {
                let plen = self.torrent.piece_length();
                let total = self.torrent.total_length();
                let req = req_index_from_piece_and_offset(index, begin, plen);
                let _ = total;
                // Upload-side cancel: nothing queued yet in this simplified path.
                let _ = req;
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
                    for i in 0..n {
                        if self.state.peer_pieces.insert(i) {
                            self.torrent.note_availability(i, 1);
                            self.torrent.note_peer_piece(self.key(), i, true);
                        }
                    }
                    self.state.peer_sent_have_all = true;
                    self.state.need_request_update = true;
                    self.torrent.bump_change();
                    self.update_requests(handle);
                }
            }
            Message::HaveNone => {
                self.state.peer_sent_have_all = false;
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
                self.state.outstanding_requests.remove(&req);
                self.state.cancelled_requests.remove(&req);
                self.torrent.clear_chunk_requested(req);
                if self.torrent.requesting_peer(req) == Some(self.key()) {
                    self.torrent.clear_assignment(req);
                }
                if self.torrent.hedge_peer(req) == Some(self.key()) {
                    self.torrent.clear_hedge(req);
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
        if self.state.need_request_update {
            self.update_requests(handle);
        }
        self.torrent.update_peer_snapshot(self.peer_conn_info());
        Ok(())
    }

    fn on_closed(&mut self, _handle: &ConnHandle, _reason: &str) {
        self.closed.store(true, Ordering::Relaxed);
        // Drop availability
        if self.state.peer_sent_have_all {
            let n = self.torrent.num_pieces();
            for i in 0..n {
                self.torrent.note_availability(i, -1);
            }
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
    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority {
        self.0.piece_priority(piece)
    }
    fn piece_of_request(&self, req: RequestIndex) -> PieceIndex {
        block_coord_from_req_index(req, self.0.piece_length(), self.0.total_length()).piece
    }
    fn block_arrived(&self, req: RequestIndex) -> bool {
        self.0.chunk_dirty(req)
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
        let plen = self.0.piece_length() as u64;
        let chunk = u64::from(self.0.chunk_size().max(1));
        let start = (u64::from(piece) * plen) / chunk;
        let end = start + (plen / chunk).max(1);
        let mut left = 0u64;
        for r in start..end {
            if !self.0.chunk_dirty(r as RequestIndex) {
                left += chunk;
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

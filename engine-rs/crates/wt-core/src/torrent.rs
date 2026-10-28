//! Torrent state machine, piece management, and reader synchronization.

use bytes::Bytes;
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::LazyLock;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use wt_types::{
    ClientStorage, Info, InfoHash, InfoHashV2, MetaInfo, PeerId, PeerInfo, PeerSource, PieceIndex,
    PiecePriority, RequestIndex, TorrentStorage, Transport, BLOCK_SIZE,
};

use crate::arrival::ArrivalModel;
use crate::peer_conn::PeerConn;
use crate::picker::{
    block_coord_from_req_index, candidate_budget, compute_playback_pipeline_limit, focus_distance,
    piece_req_index_range, req_index_from_piece_and_offset, should_steal_urgent, ActiveRequest,
    HEDGES_PER_NUDGE, HOL_RESCUES_PER_NUDGE, HOL_WINDOW_BLOCKS, SLOW_PEER_BYTES_PER_SEC,
};
use crate::reader::PieceRange;
use crate::storage_adapter::verify_piece_hash;
use crate::tuning::current_seek_tuning;
use crate::wire::Message;
use crate::{CoreError, PieceState, Result};

// Go prioritizedPeers uses one random address-hash seed per process to
// spread equal BEP 40 priorities across endpoints instead of clustering IPs.
static PEER_ADDRESS_HASH: LazyLock<std::collections::hash_map::RandomState> =
    LazyLock::new(std::collections::hash_map::RandomState::new);

pub struct PieceEntry {
    pub index: PieceIndex,
    pub priority: PiecePriority,
    pub base_priority: PiecePriority,
    pub complete: bool,
    pub checking: bool,
    pub partial: bool,
    pub ok: bool,
    pub chunks_total: usize,
    pub chunks_received: HashSet<u32>, // offsets in piece
}

pub(crate) struct AttachInfo {
    pub remote: SocketAddr,
    pub outgoing: bool,
    pub transport: Transport,
    pub encrypted: bool,
    pub peer_id: PeerId,
    pub reserved: [u8; 8],
    pub source: PeerSource,
}

impl PieceEntry {
    pub fn new(index: PieceIndex, piece_length: i64, total_length: i64) -> Self {
        let piece_start = index as i64 * piece_length;
        let piece_end = ((index as i64 + 1) * piece_length).min(total_length);
        let actual_len = (piece_end - piece_start).max(0) as usize;
        let chunks_total = actual_len.div_ceil(BLOCK_SIZE as usize);
        Self {
            index,
            priority: PiecePriority::None,
            base_priority: PiecePriority::None,
            complete: false,
            checking: false,
            partial: false,
            ok: false,
            chunks_total,
            chunks_received: HashSet::new(),
        }
    }

    pub fn state(&self) -> PieceState {
        PieceState {
            priority: self.priority,
            complete: self.complete,
            checking: self.checking,
            partial: self.partial,
            ok: self.ok,
        }
    }
}

#[allow(clippy::type_complexity)]
pub struct TorrentInner {
    pub info_hash: InfoHash,
    pub info_hash_v2: Option<InfoHashV2>,
    pub display_name: String,
    pub info: RwLock<Option<Arc<Info>>>,
    pub metainfo_raw: RwLock<Option<MetaInfo>>,
    pub storage: RwLock<Option<Arc<dyn TorrentStorage>>>,
    pub default_storage: Arc<dyn ClientStorage>,

    pub pieces: RwLock<Vec<PieceEntry>>,
    pub book: RwLock<Option<crate::piece_state::PieceBook>>,
    pub metadata: RwLock<crate::metadata::MetadataExchange>,
    pub pex: Mutex<crate::pex::PexState>,
    pub pex_last_recv: Mutex<HashMap<SocketAddr, Instant>>,
    pub peer_listen_ports: RwLock<HashMap<SocketAddr, u16>>,
    pub hasher: Mutex<Option<crate::hashing::PieceHasherPool>>,
    pub hash_seq: AtomicU64,
    pub dht_nodes: RwLock<Vec<SocketAddr>>,
    pub peer_pieces: RwLock<HashMap<u64, HashSet<PieceIndex>>>,
    pub dirty_chunks: RwLock<HashSet<RequestIndex>>,
    pub request_state: RwLock<HashMap<RequestIndex, ActiveRequest>>,
    pub assignments: RwLock<HashMap<RequestIndex, (u64, Instant)>>,
    pub hedge_peers: RwLock<HashMap<RequestIndex, u64>>,
    pub hedges: RwLock<HashMap<RequestIndex, SocketAddr>>,

    pub conns: RwLock<HashMap<SocketAddr, PeerConn>>,
    pub known_peers: RwLock<HashSet<PeerInfo>>,
    pub client: RwLock<Weak<crate::client::ClientInner>>,
    pub dialing_peers: Mutex<HashSet<SocketAddr>>,
    /// Consecutive failed dials per addr; drives exponential re-queue backoff.
    pub swarm_diagnostics: Mutex<crate::stats::SwarmDiagnostics>,
    pub dial_fail_counts: Mutex<HashMap<SocketAddr, u8>>,
    pub conn_handles: RwLock<HashMap<SocketAddr, crate::conn::ConnHandle>>,
    pub peer_snapshots: RwLock<HashMap<SocketAddr, crate::stats::PeerConnInfo>>,
    pub choked_since: RwLock<HashMap<SocketAddr, Instant>>,
    pub peer_notify: tokio::sync::Notify,
    pub trackers: RwLock<Vec<Vec<String>>>,
    pub web_seeds: RwLock<Vec<String>>,
    pub webseed_stats: crate::stats::AtomicConnStats,

    pub request_focus: AtomicI64, // 0 = none, offset + 1
    pub interest_begin: AtomicI64,
    pub interest_end: AtomicI64,

    pub data_download_disallowed: AtomicBool,
    pub download_all_requested: AtomicBool,
    pub data_upload_disallowed: AtomicBool,
    pub max_established_conns: AtomicUsize,
    pub seeding: AtomicBool,
    pub closed: AtomicBool,

    pub change_version: AtomicU64,
    pub rescues: AtomicI64,
    pub hedges_sent: AtomicI64,
    pub chokers_dropped: AtomicI64,

    pub change_cond: Arc<Condvar>,
    pub change_mutex: Arc<Mutex<()>>,
    pub on_write_chunk_error: RwLock<Option<Arc<dyn Fn(&CoreError) + Send + Sync>>>,

    pub arrival_model: Arc<RwLock<ArrivalModel>>,
    pub reader_ranges: RwLock<HashMap<u64, PieceRange>>,
}

impl TorrentInner {
    pub fn new(
        info_hash: InfoHash,
        info_hash_v2: Option<InfoHashV2>,
        display_name: String,
        default_storage: Arc<dyn ClientStorage>,
        arrival_model: Arc<RwLock<ArrivalModel>>,
    ) -> Self {
        Self {
            info_hash,
            info_hash_v2,
            display_name,
            info: RwLock::new(None),
            metainfo_raw: RwLock::new(None),
            storage: RwLock::new(None),
            default_storage,
            pieces: RwLock::new(Vec::new()),
            book: RwLock::new(None),
            metadata: RwLock::new(crate::metadata::MetadataExchange::new(
                Some(info_hash),
                info_hash_v2,
            )),
            pex: Mutex::new(crate::pex::PexState::new()),
            pex_last_recv: Mutex::new(HashMap::new()),
            peer_listen_ports: RwLock::new(HashMap::new()),
            hasher: Mutex::new(None),
            hash_seq: AtomicU64::new(0),
            dht_nodes: RwLock::new(Vec::new()),
            peer_pieces: RwLock::new(HashMap::new()),
            dirty_chunks: RwLock::new(HashSet::new()),
            request_state: RwLock::new(HashMap::new()),
            assignments: RwLock::new(HashMap::new()),
            hedge_peers: RwLock::new(HashMap::new()),
            hedges: RwLock::new(HashMap::new()),
            conns: RwLock::new(HashMap::new()),
            known_peers: RwLock::new(HashSet::new()),
            client: RwLock::new(Weak::new()),
            dialing_peers: Mutex::new(HashSet::new()),
            swarm_diagnostics: Mutex::new(Default::default()),
            dial_fail_counts: Mutex::new(HashMap::new()),
            conn_handles: RwLock::new(HashMap::new()),
            peer_snapshots: RwLock::new(HashMap::new()),
            choked_since: RwLock::new(HashMap::new()),
            peer_notify: tokio::sync::Notify::new(),
            trackers: RwLock::new(Vec::new()),
            web_seeds: RwLock::new(Vec::new()),
            webseed_stats: crate::stats::AtomicConnStats::default(),
            request_focus: AtomicI64::new(0),
            interest_begin: AtomicI64::new(0),
            interest_end: AtomicI64::new(0),
            data_download_disallowed: AtomicBool::new(false),
            download_all_requested: AtomicBool::new(false),
            data_upload_disallowed: AtomicBool::new(false),
            max_established_conns: AtomicUsize::new(100),
            seeding: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            change_version: AtomicU64::new(1),
            rescues: AtomicI64::new(0),
            hedges_sent: AtomicI64::new(0),
            chokers_dropped: AtomicI64::new(0),
            change_cond: Arc::new(Condvar::new()),
            change_mutex: Arc::new(Mutex::new(())),
            on_write_chunk_error: RwLock::new(None),
            arrival_model,
            reader_ranges: RwLock::new(HashMap::new()),
        }
    }

    pub fn bump_change(&self) {
        self.change_version.fetch_add(1, Ordering::SeqCst);
        self.peer_notify.notify_one();
        for handle in self.conn_handles.read().values() {
            handle.wake();
        }
        let _guard = self.change_mutex.lock();
        self.change_cond.notify_all();
    }

    pub fn wake_all_connections(&self) {
        self.bump_change();
    }

    pub fn add_peers(&self, peers: Vec<PeerInfo>) -> usize {
        let high_water = self
            .client
            .read()
            .upgrade()
            .map(|c| c.config.torrent_peers_high_water)
            .unwrap_or(500)
            .max(1);
        let mut added = Vec::new();
        {
            let mut known = self.known_peers.write();
            for peer in peers {
                // Go prioritizedPeers ReplaceOrInsert keys on (trusted, prio, addrHash):
                // same address + same trust replaces in place so a later PEX sighting
                // updates Source and pex_dialed can fire when the dialer pops it.
                let prior = known
                    .iter()
                    .find(|p| p.addr == peer.addr && p.trusted == peer.trusted)
                    .cloned();
                match prior {
                    Some(old) => {
                        known.remove(&old);
                        known.insert(peer.clone());
                        if old != peer {
                            added.push(peer);
                        }
                    }
                    None => {
                        if known.insert(peer.clone()) {
                            added.push(peer);
                        }
                    }
                }
            }
            // Go DeleteMin while over high water: drop untrusted first.
            while known.len() > high_water {
                let victim = known
                    .iter()
                    .min_by_key(|p| (p.trusted, self.peer_priority(p.addr)))
                    .cloned()
                    .or_else(|| known.iter().next().cloned());
                match victim {
                    Some(p) => {
                        known.remove(&p);
                    }
                    None => break,
                }
            }
        }
        let count = added.len();
        if count > 0 {
            self.bump_change();
        }
        count
    }

    pub(crate) fn attach_connection(
        self: &Arc<Self>,
        info: AttachInfo,
        handle: crate::conn::ConnHandle,
    ) -> Result<crate::peer_conn::PeerConnHandler<Self>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(CoreError::Closed);
        }
        if info.remote != handle.remote_addr() {
            return Err(CoreError::Other("peer address mismatch".into()));
        }
        let _ = (
            info.outgoing,
            info.transport,
            info.encrypted,
            info.peer_id,
            info.reserved,
            info.source,
        );
        let mut handles = self.conn_handles.write();
        if handles.contains_key(&info.remote)
            || handles.len() >= self.max_established_conns.load(Ordering::Relaxed)
        {
            return Err(CoreError::Other(
                "duplicate peer or connection limit reached".into(),
            ));
        }
        handles.insert(info.remote, handle.clone());
        drop(handles);
        self.start_hashing();
        let mut handler = crate::peer_conn::PeerConnHandler::new(Arc::clone(self), &handle);
        handler.start(&handle);
        self.bump_change();
        Ok(handler)
    }

    pub(crate) fn start_hashing(self: &Arc<Self>) {
        let mut slot = self.hasher.lock();
        if slot.is_some() {
            return;
        }
        let workers = self
            .client
            .read()
            .upgrade()
            .map_or(2, |client| client.config.piece_hashers_per_torrent);
        let (pool, rx) = crate::hashing::PieceHasherPool::with_workers(workers);
        let torrent = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Ok(result) = rx.recv() {
                let Some(torrent) = torrent.upgrade() else {
                    break;
                };
                torrent.apply_hash_result(result);
            }
        });
        *slot = Some(pool);
    }

    fn apply_hash_result(&self, result: crate::hashing::HashResult) {
        let Some(info) = self.info.read().clone() else {
            return;
        };
        let Some(storage) = self.storage.read().clone() else {
            return;
        };
        let piece = result.piece_index;
        if piece >= info.num_pieces() {
            return;
        }
        let ps = storage.piece(info.piece(piece), info.piece_hash_v1(piece));
        let valid = result.correct
            && !result.cancelled
            && result.error.is_none()
            && ps.mark_complete().is_ok();
        if !valid {
            let _ = ps.mark_not_complete();
        }
        {
            let mut pieces = self.pieces.write();
            if let Some(entry) = pieces.get_mut(piece as usize) {
                entry.checking = false;
                entry.complete = valid;
                entry.ok = true;
                if !valid {
                    entry.partial = false;
                    entry.chunks_received.clear();
                }
            }
        }
        if let Some(book) = self.book.write().as_mut() {
            book.set_complete_cached(piece, true, valid);
            if !valid {
                let (begin, end) =
                    piece_req_index_range(piece, info.piece_length, info.total_length());
                let mut dirty = self.dirty_chunks.write();
                for req in begin..end {
                    dirty.remove(&req);
                    book.mark_chunk_pending(req);
                }
            }
        }
        if valid {
            for handle in self.conn_handles.read().values() {
                handle.send_message(wt_wire::message::Message::Have { index: piece });
            }
        }
        self.bump_change();
    }

    fn peer_priority(&self, remote: SocketAddr) -> u32 {
        let client = self.client.read().upgrade();
        let local = client.as_ref().and_then(|c| c.listen_addrs.read().iter()
            .copied().find(|addr| addr.is_ipv4() == remote.is_ipv4()))
            .unwrap_or_else(|| if remote.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }.parse().unwrap());
        wt_discovery::bep40_priority(local, remote)
    }

    /// Go maxHalfOpen: five exploratory slots beyond established headroom.
    pub(crate) fn max_half_open(&self, configured: usize) -> usize {
        let handles = self.conn_handles.read();
        let maximum = self.max_established_conns.load(Ordering::Relaxed) as i64;
        let incoming = handles.values().filter(|h| !h.is_outgoing()).count() as i64;
        (maximum - handles.len() as i64 + 5.max(incoming - maximum / 2))
            .max(0).min(configured as i64) as usize
    }

    pub(crate) fn wants_conns(&self) -> usize {
        if self.closed.load(Ordering::Relaxed) {
            return 0;
        }
        self.max_established_conns
            .load(Ordering::Relaxed)
            .saturating_sub(self.conn_handles.read().len())
    }

    pub(crate) fn take_dial_candidates(&self, max: usize) -> Vec<PeerInfo> {
        // Go prioritizedPeers.PopMax: trust, then BEP 40 priority.
        let handles = self.conn_handles.read();
        let mut dialing = self.dialing_peers.lock();
        let mut known = self.known_peers.write();
        let mut eligible: Vec<PeerInfo> = known
            .iter()
            .filter(|p| {
                !handles.contains_key(&p.addr)
                    && !dialing.contains(&p.addr)
            })
            .cloned()
            .collect();
        eligible.sort_by_cached_key(|p| std::cmp::Reverse((
            p.trusted, self.peer_priority(p.addr), PEER_ADDRESS_HASH.hash_one(p.addr.to_string()),
        )));
        // The same endpoint can arrive from tracker, DHT and PEX. Go checks
        // connectingToPeerAddr before starting each attempt; reserve it once.
        let mut seen = HashSet::new();
        eligible.retain(|p| seen.insert(p.addr));
        eligible.truncate(max);
        for p in &eligible {
            known.retain(|x| x.addr != p.addr);
            dialing.insert(p.addr);
        }
        eligible
    }

    pub(crate) fn dial_finished(&self, addr: SocketAddr, ok: bool) {
        self.dialing_peers.lock().remove(&addr);
        if ok {
            self.dial_fail_counts.lock().remove(&addr);
        } else {
            let mut counts = self.dial_fail_counts.lock();
            let count = counts.entry(addr).or_insert(0);
            *count = count.saturating_add(1);
        }
        self.bump_change();
    }

    pub(crate) fn peers_changed(&self) -> &tokio::sync::Notify {
        &self.peer_notify
    }

    pub fn set_info(&self, info: Arc<Info>) -> Result<()> {
        let num_pieces = info.num_pieces() as usize;
        let mut piece_entries = Vec::with_capacity(num_pieces);
        let piece_len = info.piece_length;
        let total_len = info.total_length();
        for i in 0..num_pieces {
            let mut entry = PieceEntry::new(i as PieceIndex, piece_len, total_len);
            if self.download_all_requested.load(Ordering::Relaxed) {
                entry.priority = PiecePriority::Normal;
                entry.base_priority = PiecePriority::Normal;
            }
            piece_entries.push(entry);
        }

        // Open storage
        let storage = self.default_storage.open_torrent(&info, self.info_hash)?;
        for entry in &mut piece_entries {
            let completion = storage
                .piece(info.piece(entry.index), info.piece_hash_v1(entry.index))
                .completion();
            entry.ok = completion.ok;
            entry.complete = completion.ok && completion.complete;
        }
        *self.storage.write() = Some(storage);
        *self.pieces.write() = piece_entries;
        crate::torrent_impls::init_book_and_glue(self, &info);
        *self.info.write() = Some(info);
        self.bump_change();
        Ok(())
    }

    pub fn piece_request_index_begin(&self, piece: PieceIndex) -> RequestIndex {
        let info_guard = self.info.read();
        if let Some(info) = &*info_guard {
            let (begin, _) = piece_req_index_range(piece, info.piece_length, info.total_length());
            begin
        } else {
            0
        }
    }

    pub fn request_focus_index(&self) -> Option<RequestIndex> {
        let val = self.request_focus.load(Ordering::Relaxed);
        if val <= 0 {
            return None;
        }
        let off = val - 1;
        let info = self.info.read();
        let info = info.as_ref()?;
        if off < 0 || off >= info.total_length() {
            return None;
        }
        let piece = (off / info.piece_length) as PieceIndex;
        let in_piece_offset = (off % info.piece_length) as u32;
        let req_in_piece = in_piece_offset / BLOCK_SIZE;
        let (start, _) = piece_req_index_range(piece, info.piece_length, info.total_length());
        Some(start + req_in_piece)
    }

    pub fn is_block_arrived(&self, req: RequestIndex) -> bool {
        if self.dirty_chunks.read().contains(&req) {
            return true;
        }
        let info_guard = self.info.read();
        if let Some(info) = &*info_guard {
            let coord = block_coord_from_req_index(req, info.piece_length, info.total_length());
            let pieces = self.pieces.read();
            if let Some(p) = pieces.get(coord.piece as usize) {
                return p.complete;
            }
        }
        false
    }

    pub fn max_end_request(&self) -> RequestIndex {
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return 0;
        };
        let num_pieces = info.pieces.len() / 20;
        if num_pieces == 0 {
            return 0;
        }
        let last_piece = (num_pieces - 1) as u32;
        let (_, end_req) =
            piece_req_index_range(last_piece, info.piece_length, info.total_length());
        end_req
    }

    pub fn head_of_line(&self) -> Option<RequestIndex> {
        let focus = self.request_focus_index()?;
        let total_reqs = self.max_end_request();
        let end = (focus + HOL_WINDOW_BLOCKS).min(total_reqs);
        (focus..end).find(|&r| !self.is_block_arrived(r))
    }

    pub fn rescue_head_of_line(&self) -> usize {
        let tuning = current_seek_tuning();
        if !tuning.hol_rescue {
            return 0;
        }
        let Some(head) = self.head_of_line() else {
            return 0;
        };
        let Some(focus) = self.request_focus_index() else {
            return 0;
        };
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return 0;
        };

        let total_reqs = self.max_end_request();
        let end = (focus + HOL_WINDOW_BLOCKS).min(total_reqs);
        let after = Duration::from_secs_f64(tuning.hol_after_ms / 1000.0);
        let mut moved = 0;

        let mut req_state_lock = self.request_state.write();
        let conns_lock = self.conns.read();

        for r in head..end {
            if moved >= HOL_RESCUES_PER_NUDGE {
                break;
            }
            if self.is_block_arrived(r) {
                continue;
            }
            let Some(holder_state) = req_state_lock.get(&r) else {
                continue;
            };
            let Some(holder_conn) = conns_lock.get(&holder_state.peer) else {
                continue;
            };

            let holder_rtt = holder_conn.inner.read().rtt.estimate;
            if holder_state.when.elapsed() < after.max(2 * holder_rtt) {
                continue;
            }

            let coord = block_coord_from_req_index(r, info.piece_length, info.total_length());
            let piece = coord.piece;

            let mut best_conn: Option<PeerConn> = None;
            let mut best_arrival = f64::MAX;
            let holder_arrival =
                holder_conn.expected_arrival(BLOCK_SIZE as usize, tuning.learned_arrival, None);

            for conn in conns_lock.values() {
                if conn.inner.read().remote_addr == holder_state.peer || conn.is_closed() {
                    continue;
                }
                let c_inner = conn.inner.read();
                if c_inner.peer_choking || !c_inner.peer_pieces.contains(&piece) {
                    continue;
                }
                drop(c_inner);
                let arrival =
                    conn.expected_arrival(BLOCK_SIZE as usize, tuning.learned_arrival, None);
                if arrival < best_arrival {
                    best_arrival = arrival;
                    best_conn = Some(conn.clone());
                }
            }

            if let Some(best) = best_conn {
                if best_arrival < holder_arrival {
                    // Steal/rescue
                    holder_conn.send_message(Message::Cancel {
                        index: piece,
                        begin: coord.offset,
                        length: coord.length,
                    });
                    best.send_message(Message::Request {
                        index: piece,
                        begin: coord.offset,
                        length: coord.length,
                    });
                    req_state_lock.insert(
                        r,
                        ActiveRequest {
                            peer: best.inner.read().remote_addr,
                            when: Instant::now(),
                            cancelled: false,
                        },
                    );
                    self.rescues.fetch_add(1, Ordering::Relaxed);
                    moved += 1;
                }
            }
        }

        moved
    }

    pub fn hedge_head_of_line(&self) -> usize {
        let tuning = current_seek_tuning();
        if !tuning.hedge {
            return 0;
        }
        let Some(head) = self.head_of_line() else {
            return 0;
        };
        let Some(focus) = self.request_focus_index() else {
            return 0;
        };
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return 0;
        };

        let total_reqs = self.max_end_request();
        let end = (focus + HOL_WINDOW_BLOCKS).min(total_reqs);
        let after = Duration::from_secs_f64(tuning.hol_after_ms / 1000.0);
        let mut sent = 0;

        let req_state_lock = self.request_state.read();
        let mut hedges_lock = self.hedges.write();
        let conns_lock = self.conns.read();

        for r in head..end {
            if sent >= HEDGES_PER_NUDGE {
                break;
            }
            if self.is_block_arrived(r) {
                continue;
            }
            let Some(holder_state) = req_state_lock.get(&r) else {
                continue;
            };
            if hedges_lock.contains_key(&r) {
                continue;
            }
            let Some(holder_conn) = conns_lock.get(&holder_state.peer) else {
                continue;
            };

            let overdue = if tuning.learned_arrival {
                holder_conn.block_overdue(r, Duration::from_millis(100), true)
            } else {
                let rtt = holder_conn.inner.read().rtt.estimate;
                holder_state.when.elapsed() >= after.max(2 * rtt)
            };

            if !overdue {
                continue;
            }

            let coord = block_coord_from_req_index(r, info.piece_length, info.total_length());
            let piece = coord.piece;

            let mut best_conn: Option<PeerConn> = None;
            let mut best_arrival = f64::MAX;

            for conn in conns_lock.values() {
                if conn.inner.read().remote_addr == holder_state.peer || conn.is_closed() {
                    continue;
                }
                let c_inner = conn.inner.read();
                if c_inner.peer_choking || !c_inner.peer_pieces.contains(&piece) {
                    continue;
                }
                drop(c_inner);
                let arrival =
                    conn.expected_arrival(BLOCK_SIZE as usize, tuning.learned_arrival, None);
                if arrival < best_arrival {
                    best_arrival = arrival;
                    best_conn = Some(conn.clone());
                }
            }

            if let Some(best) = best_conn {
                best.send_message(Message::Request {
                    index: piece,
                    begin: coord.offset,
                    length: coord.length,
                });
                hedges_lock.insert(r, best.inner.read().remote_addr);
                self.hedges_sent.fetch_add(1, Ordering::Relaxed);
                sent += 1;
            }
        }

        sent
    }

    pub fn nudge_urgent_requests(&self) -> usize {
        if self.closed.load(Ordering::Relaxed)
            || self.data_download_disallowed.load(Ordering::Relaxed)
        {
            return 0;
        }
        for handle in self.conn_handles.read().values() {
            handle.wake();
        }
        self.rescue_head_of_line();
        self.hedge_head_of_line();

        let pieces = self.pieces.read();
        let urgent_pieces: Vec<PieceIndex> = pieces
            .iter()
            .filter(|p| !p.complete && p.priority >= PiecePriority::Now)
            .map(|p| p.index)
            .collect();
        drop(pieces);

        if urgent_pieces.is_empty() {
            return 0;
        }

        let eligible: Vec<PeerConn> = {
            let conns = self.conns.read();
            conns
                .values()
                .filter_map(|conn| {
                    if conn.is_closed() {
                        return None;
                    }
                    let c_inner = conn.inner.read();
                    if c_inner.outstanding_requests.len() >= c_inner.max_requests {
                        return None;
                    }
                    let has_urgent = urgent_pieces.iter().any(|&i| {
                        (c_inner.peer_pieces.contains(&i) || c_inner.peer_sent_have_all)
                            && (!c_inner.peer_choking || c_inner.peer_allowed_fast.contains(&i))
                    });
                    if has_urgent {
                        Some(conn.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        let mut nudged = 0;
        for conn in eligible {
            self.refill_peer_requests(&conn);
            nudged += 1;
        }
        nudged
    }

    pub fn refill_peer_requests(&self, conn: &PeerConn) {
        if self.closed.load(Ordering::Relaxed)
            || self.data_download_disallowed.load(Ordering::Relaxed)
            || conn.is_closed()
        {
            return;
        }
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return;
        };

        let tuning = current_seek_tuning();
        let c_inner = conn.inner.read();
        if c_inner.peer_choking && c_inner.peer_allowed_fast.is_empty() {
            return;
        }

        let pipeline_limit = compute_playback_pipeline_limit(
            c_inner.download_rate.rate(),
            c_inner.rtt.estimate,
            self.request_focus.load(Ordering::Relaxed) > 0,
        );
        // Match Go `nominalMaxRequests`: peak*2 and LTEP reqq, with pipeline
        // as an extra cap. Never use bare max_requests (250) — that flooded
        // seeds with 100–228 outstanding and left downloaded=0.
        const NOMINAL_CEILING: usize = crate::peer_conn::MAX_LOCAL_TO_REMOTE_REQUESTS;
        let peak_cap = c_inner
            .peak_requests
            .max(1)
            .saturating_mul(2)
            .clamp(1, NOMINAL_CEILING);
        let max_reqs = if pipeline_limit > 0 {
            pipeline_limit.min(peak_cap).min(c_inner.max_requests.max(1))
        } else {
            peak_cap.min(c_inner.max_requests.max(1))
        };

        if c_inner.outstanding_requests.len() >= max_reqs {
            return;
        }

        let slots_available = max_reqs - c_inner.outstanding_requests.len();
        let budget = candidate_budget(max_reqs);
        let budget_limit = if budget > 0 { budget } else { usize::MAX };
        drop(c_inner);

        let focus_opt = self.request_focus_index();
        let sorted_pieces: Vec<(PieceIndex, PiecePriority)> = {
            let pieces_guard = self.pieces.read();
            let mut list: Vec<(PieceIndex, PiecePriority)> = pieces_guard
                .iter()
                .filter(|p| !p.complete && p.priority > PiecePriority::None)
                .map(|p| (p.index, p.priority))
                .collect();
            drop(pieces_guard);

            let focus_piece = focus_opt
                .map(|r| {
                    let coord =
                        block_coord_from_req_index(r, info.piece_length, info.total_length());
                    coord.piece
                })
                .unwrap_or(0);

            list.sort_by(|a, b| {
                // First by priority descending
                b.1.cmp(&a.1).then_with(|| {
                    // If priority is Now, order by distance from playhead
                    if a.1 >= PiecePriority::Now {
                        focus_distance(a.0, focus_piece).cmp(&focus_distance(b.0, focus_piece))
                    } else {
                        a.0.cmp(&b.0)
                    }
                })
            });
            list
        };

        let mut sent = 0;
        let mut candidates_scanned = 0;
        let mut req_state_lock = self.request_state.write();
        let urgent_steal_age = Duration::from_secs(1); // default 1s for urgent stealing

        if !conn.inner.read().am_interested {
            conn.inner.write().am_interested = true;
            conn.send_message(Message::Interested);
        }

        for (piece, priority) in sorted_pieces {
            if sent >= slots_available || candidates_scanned >= budget_limit {
                break;
            }
            let c_inner = conn.inner.read();
            if !c_inner.peer_pieces.contains(&piece) && !c_inner.peer_sent_have_all {
                continue;
            }
            if c_inner.peer_choking && !c_inner.peer_allowed_fast.contains(&piece) {
                continue;
            }

            // SlowPeerYield: skip only non-urgent Next pieces, and only after
            // this peer has already delivered useful data (matches peer_conn).
            // `>= Next` previously starved new peers on Now at rate=0.
            if tuning.slow_peer_yield
                && priority == PiecePriority::Next
                && c_inner.last_useful_chunk_received.is_some()
                && c_inner.download_rate.rate() < SLOW_PEER_BYTES_PER_SEC
            {
                let conns_lock = self.conns.read();
                let faster_exists = conns_lock.values().any(|other| {
                    if other.inner.read().remote_addr == c_inner.remote_addr || other.is_closed() {
                        return false;
                    }
                    let o_inner = other.inner.read();
                    !o_inner.peer_choking
                        && (o_inner.peer_pieces.contains(&piece) || o_inner.peer_sent_have_all)
                        && o_inner.download_rate.rate() > SLOW_PEER_BYTES_PER_SEC
                });
                drop(conns_lock);
                if faster_exists {
                    continue;
                }
            }
            drop(c_inner);

            let (start_req, end_req) =
                piece_req_index_range(piece, info.piece_length, info.total_length());
            for r in start_req..end_req {
                if sent >= slots_available || candidates_scanned >= budget_limit {
                    break;
                }
                candidates_scanned += 1;
                if self.is_block_arrived(r) {
                    continue;
                }

                let coord = block_coord_from_req_index(r, info.piece_length, info.total_length());

                // Check if already requested
                if let Some(active) = req_state_lock.get(&r) {
                    if active.peer == conn.inner.read().remote_addr {
                        continue;
                    }
                    // Check urgent stealing
                    let conns_lock = self.conns.read();
                    let existing_last_useful = conns_lock
                        .get(&active.peer)
                        .and_then(|c| c.inner.read().last_useful_chunk_received);
                    let stealer_last_useful = conn.inner.read().last_useful_chunk_received;
                    drop(conns_lock);

                    if should_steal_urgent(
                        r,
                        priority,
                        existing_last_useful,
                        stealer_last_useful,
                        active.when,
                        urgent_steal_age,
                        focus_opt,
                    ) {
                        // Cancel from existing peer
                        let conns_lock = self.conns.read();
                        if let Some(existing) = conns_lock.get(&active.peer) {
                            existing.send_message(Message::Cancel {
                                index: piece,
                                begin: coord.offset,
                                length: coord.length,
                            });
                        }
                        drop(conns_lock);

                        conn.send_message(Message::Request {
                            index: piece,
                            begin: coord.offset,
                            length: coord.length,
                        });
                        conn.note_request_sent(r, BLOCK_SIZE as usize, None);
                        req_state_lock.insert(
                            r,
                            ActiveRequest {
                                peer: conn.inner.read().remote_addr,
                                when: Instant::now(),
                                cancelled: false,
                            },
                        );
                        sent += 1;
                    }
                } else {
                    // Not requested yet!
                    conn.send_message(Message::Request {
                        index: piece,
                        begin: coord.offset,
                        length: coord.length,
                    });
                    conn.note_request_sent(r, BLOCK_SIZE as usize, None);
                    req_state_lock.insert(
                        r,
                        ActiveRequest {
                            peer: conn.inner.read().remote_addr,
                            when: Instant::now(),
                            cancelled: false,
                        },
                    );
                    sent += 1;
                }
            }
        }
    }

    pub fn on_block_received(
        &self,
        from_peer: SocketAddr,
        piece: PieceIndex,
        begin: u32,
        data: Bytes,
    ) {
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return;
        };

        let storage_guard = self.storage.read();
        let Some(storage) = &*storage_guard else {
            return;
        };

        let piece_obj = info.piece(piece);
        let piece_storage = storage.piece(piece_obj, info.piece_hash_v1(piece));

        // Write block to storage
        if let Err(err) = piece_storage.write_at(&data, begin as i64) {
            let error = CoreError::Storage(err);
            if let Some(cb) = &*self.on_write_chunk_error.read() {
                cb(&error);
            }
            return;
        }

        let req_idx = req_index_from_piece_and_offset(piece, begin, info.piece_length);

        // Update stats
        self.dirty_chunks.write().insert(req_idx);
        self.request_state.write().remove(&req_idx);

        let conns = self.conns.read();
        if let Some(conn) = conns.get(&from_peer) {
            conn.note_block_received(req_idx, Some(&mut *self.arrival_model.write()));
            conn.inner
                .read()
                .stats
                .bytes_read_data
                .fetch_add(data.len() as i64, Ordering::Relaxed);
            conn.inner
                .read()
                .stats
                .bytes_read_useful_data
                .fetch_add(data.len() as i64, Ordering::Relaxed);
            conn.inner
                .read()
                .stats
                .chunks_read
                .fetch_add(1, Ordering::Relaxed);
            conn.inner
                .read()
                .stats
                .chunks_read_useful
                .fetch_add(1, Ordering::Relaxed);
        }

        // Cancel hedge if hedged
        if let Some(hedged_peer) = self.hedges.write().remove(&req_idx) {
            if hedged_peer != from_peer {
                if let Some(other_conn) = conns.get(&hedged_peer) {
                    other_conn.send_message(Message::Cancel {
                        index: piece,
                        begin,
                        length: data.len() as u32,
                    });
                }
            }
        }
        drop(conns);

        // Check if piece is complete
        let mut pieces_lock = self.pieces.write();
        if let Some(piece_entry) = pieces_lock.get_mut(piece as usize) {
            piece_entry.partial = true;
            piece_entry.chunks_received.insert(begin);
            if piece_entry.chunks_received.len() >= piece_entry.chunks_total {
                piece_entry.checking = true;
                let expected_hash = info.piece_hash_v1(piece);
                let valid =
                    verify_piece_hash(&*piece_storage, piece_obj.length as usize, expected_hash);
                piece_entry.checking = false;
                if valid {
                    let _ = piece_storage.mark_complete();
                    piece_entry.complete = true;
                    piece_entry.ok = true;
                    drop(pieces_lock);
                    // Broadcast Have to all peers
                    let conns = self.conns.read();
                    for conn in conns.values() {
                        conn.send_message(Message::Have { index: piece });
                    }
                } else {
                    let _ = piece_storage.mark_not_complete();
                    piece_entry.complete = false;
                    piece_entry.ok = false;
                    piece_entry.chunks_received.clear();
                    piece_entry.partial = false;
                }
            }
        }

        self.bump_change();
    }
}

#[cfg(test)]
mod dial_parity_tests {
    use super::*;

    fn torrent() -> TorrentInner {
        TorrentInner::new(InfoHash([7; 20]), None, "dial-parity".into(),
            Arc::new(crate::storage_adapter::MemoryClientStorage::new()),
            Arc::new(RwLock::new(crate::arrival::ArrivalModel::default())))
    }

    #[test]
    fn half_open_attempts_do_not_hide_demand_for_established_peers() {
        let t = torrent();
        t.max_established_conns.store(2, Ordering::Relaxed);
        t.dialing_peers.lock().extend([
            "192.0.2.1:6881".parse::<SocketAddr>().unwrap(), "192.0.2.2:6881".parse::<SocketAddr>().unwrap(),
        ]);
        assert_eq!(t.wants_conns(), 2, "Go wantAnyConns counts established peers only");
    }

    #[test]
    fn one_address_from_multiple_sources_uses_only_one_dial_slot() {
        let t = torrent();
        let first: SocketAddr = "192.0.2.1:6881".parse().unwrap();
        let second: SocketAddr = "192.0.2.2:6881".parse().unwrap();
        let mut peer = PeerInfo { id: None, addr: first, source: wt_types::PeerSource::Direct,
            supports_encryption: false, pex: Default::default(), trusted: true };
        t.add_peers(vec![peer.clone()]);
        peer.source = wt_types::PeerSource::Pex;
        t.add_peers(vec![peer.clone()]);
        assert_eq!(
            t.known_peers.read().len(),
            1,
            "Go prioritizedPeers replaces same addr+trust from another source"
        );
        peer.addr = second;
        peer.trusted = false;
        t.add_peers(vec![peer]);
        let candidates = t.take_dial_candidates(2);
        assert_eq!(candidates.iter().map(|p| p.addr).collect::<HashSet<_>>().len(), 2,
            "duplicate discovery must not spend half-open slots on the same address");
    }

    #[test]
    fn dial_pool_uses_trust_then_bep40_priority_even_after_failures() {
        let t = torrent();
        let local = "0.0.0.0:0".parse().unwrap();
        let mut peers: Vec<PeerInfo> = (1..=8).map(|n| PeerInfo {
            id: None, addr: format!("192.0.2.{n}:6881").parse().unwrap(),
            source: wt_types::PeerSource::Direct, supports_encryption: false,
            pex: Default::default(), trusted: true,
        }).collect();
        peers.sort_by_key(|p| std::cmp::Reverse(wt_discovery::bep40_priority(local, p.addr)));
        let expected: Vec<_> = peers.iter().map(|p| p.addr).collect();
        // Go does not lower the priority of a rediscovered peer after a dial failure.
        t.dial_fail_counts.lock().insert(expected[0], 7);
        t.add_peers(peers);
        let got: Vec<_> = t.take_dial_candidates(8).iter().map(|p| p.addr).collect();
        assert_eq!(got.iter().map(|addr| wt_discovery::bep40_priority(local, *addr)).collect::<Vec<_>>(),
            expected.iter().map(|addr| wt_discovery::bep40_priority(local, *addr)).collect::<Vec<_>>());
    }

    #[test]
    fn pex_ingest_reaches_dial_candidates_with_pex_source() {
        use crate::peer_conn::PeerTorrent;
        use std::net::SocketAddrV4;
        use wt_wire::ext::PexMsg;

        let t = torrent();
        let from: SocketAddr = "198.51.100.1:6881".parse().unwrap();
        let added: SocketAddrV4 = "203.0.113.10:51413".parse().unwrap();
        let msg = PexMsg {
            added: vec![added],
            added_flags: vec![Default::default()],
            ..Default::default()
        };
        t.ingest_pex(from, &msg).unwrap();
        assert_eq!(t.swarm_diagnostics.lock().pex_received, 1);

        let candidates = t.take_dial_candidates(8);
        assert_eq!(candidates.len(), 1, "PEX added peers must enter the dial pool: {candidates:?}");
        assert_eq!(candidates[0].addr, SocketAddr::V4(added));
        assert_eq!(
            candidates[0].source,
            PeerSource::Pex,
            "dial path tags pex_dialed from PeerSource::Pex"
        );
        // Mirror client dialer: selected PEX candidates increment pex_dialed.
        if candidates[0].source == PeerSource::Pex {
            t.swarm_diagnostics.lock().pex_dialed += 1;
        }
        assert_eq!(t.swarm_diagnostics.lock().pex_dialed, 1);
    }

    #[test]
    fn pex_replaces_same_addr_untrusted_discovery_like_go() {
        // Go prioritizedPeers ReplaceOrInsert keys on (trusted, prio, addrHash):
        // a later PEX sighting of an untrusted DHT addr must dial as Pex.
        let t = torrent();
        let addr: SocketAddr = "203.0.113.50:6881".parse().unwrap();
        t.add_peers(vec![PeerInfo {
            id: None,
            addr,
            source: PeerSource::DhtGetPeers,
            supports_encryption: false,
            pex: Default::default(),
            trusted: false,
        }]);
        t.add_peers(vec![PeerInfo {
            id: None,
            addr,
            source: PeerSource::Pex,
            supports_encryption: false,
            pex: Default::default(),
            trusted: false,
        }]);
        let candidates = t.take_dial_candidates(4);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].source,
            PeerSource::Pex,
            "Go replaces same-(addr,trusted) so PEX wins over DHT for pex_dialed"
        );
    }
}

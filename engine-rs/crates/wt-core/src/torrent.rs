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
    /// Not-before instant for re-queued dial failures (exp backoff).
    pub dial_backoff: Mutex<HashMap<SocketAddr, Instant>>,
    /// Consecutive failed dials per addr; drives exponential re-queue backoff
    /// and dial-candidate ordering (fresh before recirculated dead contacts).
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
            dial_backoff: Mutex::new(HashMap::new()),
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
            info.transport,
            info.encrypted,
            info.peer_id,
            info.reserved,
            info.source,
        );
        // Duplicate remote: reject (Go closes the new one / prefers network).
        if self.conn_handles.read().contains_key(&info.remote) {
            return Err(CoreError::Other("duplicate peer".into()));
        }
        let max = self.max_established_conns.load(Ordering::Relaxed);
        if self.conn_handles.read().len() >= max {
            // Go: replace worstBadConn when at maxEstablishedConns.
            let outgoing = self
                .conn_handles
                .read()
                .values()
                .filter(|h| h.is_outgoing())
                .count();
            let established = self.conn_handles.read().len();
            let incoming = established.saturating_sub(outgoing);
            let incoming_is_bad = (incoming.saturating_sub(outgoing) > 1) && info.outgoing;
            let outgoing_is_bad = (outgoing.saturating_sub(incoming) > 1) && !info.outgoing;
            let Some(victim) = self.worst_bad_conn_addr(incoming_is_bad, outgoing_is_bad) else {
                return Err(CoreError::Other("don't want conn".into()));
            };
            self.drop_conn_addr(victim, "replaced by better peer");
        }
        let mut handles = self.conn_handles.write();
        if handles.len() >= max {
            // Race: another attach won the slot.
            return Err(CoreError::Other("don't want conn".into()));
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

    /// Go `wantAnyConns` / `wantOutgoingConns`: under the established cap return
    /// free slots; at the cap still return 1 when a bad conn can be replaced
    /// (so openNewConns can dial a replacement like Go).
    pub(crate) fn wants_conns(&self) -> usize {
        if self.closed.load(Ordering::Relaxed) {
            return 0;
        }
        let max = self.max_established_conns.load(Ordering::Relaxed);
        let established = self.conn_handles.read().len();
        if established < max {
            return max - established;
        }
        let outgoing = self
            .conn_handles
            .read()
            .values()
            .filter(|h| h.is_outgoing())
            .count();
        let incoming = established.saturating_sub(outgoing);
        // Go wantOutgoingConns at max: prefer replacing excess incoming.
        let incoming_is_bad = incoming.saturating_sub(outgoing) > 1;
        if self
            .worst_bad_conn_addr(incoming_is_bad, false)
            .is_some()
        {
            1
        } else {
            0
        }
    }

    /// Go `PeerConn.useful`: metadata peer, seeding interest, or has wanted pieces.
    fn conn_is_useful(&self, addr: SocketAddr) -> bool {
        let conns = self.conns.read();
        let Some(c) = conns.get(&addr) else {
            return false;
        };
        let inner = c.inner.read();
        if c.closed.load(Ordering::Relaxed) {
            return false;
        }
        if self.info.read().is_none() {
            return inner.peer_ext_ut_metadata.is_some();
        }
        if self.seeding.load(Ordering::Relaxed) && inner.peer_interested {
            return true;
        }
        // Approximate peerHasWantedPieces: any piece we lack that they have.
        let pieces = self.pieces.read();
        if inner.peer_sent_have_all {
            return pieces.iter().any(|p| !p.complete);
        }
        inner.peer_pieces.iter().any(|&p| {
            pieces
                .get(p as usize)
                .map(|e| !e.complete)
                .unwrap_or(true)
        })
    }

    /// Go `worstBadConn` / `worstBadConnFromSlice`: pick a replaceable bad
    /// connection (bad direction, wasted chunks, or aged worse-half).
    pub(crate) fn worst_bad_conn_addr(
        &self,
        incoming_is_bad: bool,
        outgoing_is_bad: bool,
    ) -> Option<SocketAddr> {
        let handles = self.conn_handles.read();
        if handles.is_empty() {
            return None;
        }
        let max = self.max_established_conns.load(Ordering::Relaxed);
        let conns = self.conns.read();

        struct Cand {
            addr: SocketAddr,
            bad_direction: bool,
            useful: bool,
            last_helpful: Option<Instant>,
            completed_handshake: Instant,
            wasted: i64,
            useful_chunks: i64,
            peer_prio: u32,
        }

        let mut cands: Vec<Cand> = Vec::with_capacity(handles.len());
        for (addr, handle) in handles.iter() {
            if handle.is_closed() {
                continue;
            }
            let outgoing = handle.is_outgoing();
            let bad_direction = (incoming_is_bad && !outgoing) || (outgoing_is_bad && outgoing);
            let stats = handle.stats().snapshot();
            let (last_helpful, _) = conns
                .get(addr)
                .map(|c| {
                    let i = c.inner.read();
                    (i.last_useful_chunk_received, i.connected_at)
                })
                .unwrap_or((None, handle.connected_at()));
            cands.push(Cand {
                addr: *addr,
                bad_direction,
                useful: self.conn_is_useful(*addr),
                last_helpful,
                completed_handshake: handle.connected_at(),
                wasted: stats.chunks_read_wasted,
                useful_chunks: stats.chunks_read_useful,
                peer_prio: self.peer_priority(*addr),
            });
        }
        drop(conns);
        drop(handles);

        // worseConnInput.Less / heap order: worst first (bad-dir, !useful,
        // older helpful, older handshake, lower peer priority).
        cands.sort_by(|a, b| {
            use std::cmp::Ordering as O;
            // bad_direction true is worse → should come first
            match (a.bad_direction, b.bad_direction) {
                (true, false) => return O::Less,
                (false, true) => return O::Greater,
                _ => {}
            }
            // useful false is worse
            match (a.useful, b.useful) {
                (false, true) => return O::Less,
                (true, false) => return O::Greater,
                _ => {}
            }
            // older (smaller) last_helpful is worse; None is oldest
            match (a.last_helpful, b.last_helpful) {
                (None, Some(_)) => return O::Less,
                (Some(_), None) => return O::Greater,
                (Some(la), Some(lb)) => {
                    if la != lb {
                        return la.cmp(&lb); // earlier = worse = Less
                    }
                }
                (None, None) => {}
            }
            // older handshake is worse
            match a.completed_handshake.cmp(&b.completed_handshake) {
                O::Equal => {}
                o => return o,
            }
            match a.peer_prio.cmp(&b.peer_prio) {
                O::Equal => {}
                o => return o,
            }
            // stable tie-break on addr bytes
            a.addr.to_string().cmp(&b.addr.to_string())
        });

        let total = cands.len();
        for (idx, c) in cands.iter().enumerate() {
            if c.bad_direction {
                return Some(c.addr);
            }
            if c.wasted >= 6 && c.wasted > c.useful_chunks {
                return Some(c.addr);
            }
            // Remaining after this pop would be total-1-idx; Go checks
            // `wcs.Len() >= (max+1)/2` AFTER popping, i.e. remaining >= half.
            let remaining_after = total - 1 - idx;
            if remaining_after >= max.div_ceil(2)
                && c.completed_handshake.elapsed() > Duration::from_secs(60)
            {
                return Some(c.addr);
            }
        }
        None
    }

    /// Absolute worst connection by Go worseConn ordering (for SetMaxEstablishedConns).
    pub(crate) fn worst_conn_addr(&self) -> Option<SocketAddr> {
        let handles = self.conn_handles.read();
        if handles.is_empty() {
            return None;
        }
        let conns = self.conns.read();
        let mut best: Option<(SocketAddr, bool, bool, Option<Instant>, Instant, u32)> = None;
        for (addr, handle) in handles.iter() {
            if handle.is_closed() {
                continue;
            }
            let useful = self.conn_is_useful(*addr);
            let last_helpful = conns
                .get(addr)
                .and_then(|c| c.inner.read().last_useful_chunk_received);
            let completed = handle.connected_at();
            let prio = self.peer_priority(*addr);
            let cand = (*addr, false, useful, last_helpful, completed, prio);
            let replace = match &best {
                None => true,
                Some((_, _, bu, blh, bc, bp)) => {
                    // worse-first vs current best (which holds the worst so far)
                    match (useful, bu) {
                        (false, true) => true,
                        (true, false) => false,
                        _ => match (last_helpful, blh) {
                            (None, Some(_)) => true,
                            (Some(_), None) => false,
                            (Some(la), Some(lb)) if la != *lb => la < *lb,
                            _ => {
                                completed < *bc
                                    || (completed == *bc && prio < *bp)
                            }
                        },
                    }
                }
            };
            if replace {
                best = Some(cand);
            }
        }
        best.map(|c| c.0)
    }

    /// Close and detach a connection (Go `dropConnection` / deletePeerConn).
    /// Frees the established slot immediately; records the close reason.
    pub(crate) fn drop_conn_addr(&self, addr: SocketAddr, reason: &str) {
        if let Some(handle) = self.conn_handles.write().remove(&addr) {
            handle.close(reason);
            *self
                .swarm_diagnostics
                .lock()
                .closes
                .entry(crate::stats::failure_kind(reason).into())
                .or_default() += 1;
            eprintln!(
                "peer_closed addr={} out={} src={:?} reason={}",
                addr,
                handle.is_outgoing(),
                handle.source(),
                reason.chars().take(120).collect::<String>()
            );
        }
        self.conns.write().remove(&addr);
        self.peer_pieces
            .write()
            .remove(&crate::torrent_impls::peer_key_from_addr(addr));
        self.peer_snapshots.write().remove(&addr);
        self.choked_since.write().remove(&addr);
        self.bump_change();
    }

    pub(crate) fn take_dial_candidates(&self, max: usize) -> Vec<PeerInfo> {
        // Go prioritizedPeers.PopMax is trust then BEP 40. Pure Pop without
        // rediscovery drains the pool faster than tracker/DHT refill on Ubuntu
        // (pool 220→<40, dialFailures.timeout≈400). Failed dials are re-queued
        // with exp backoff; among ready peers prefer lower fail_counts so
        // recirculated dead contacts do not starve never-tried addrs.
        let handles = self.conn_handles.read();
        let mut dialing = self.dialing_peers.lock();
        let backoff = self.dial_backoff.lock();
        let fail_counts = self.dial_fail_counts.lock();
        let now = Instant::now();
        let mut known = self.known_peers.write();
        let mut eligible: Vec<PeerInfo> = known
            .iter()
            .filter(|p| {
                !handles.contains_key(&p.addr)
                    && !dialing.contains(&p.addr)
                    && backoff.get(&p.addr).is_none_or(|until| *until <= now)
            })
            .cloned()
            .collect();
        // The same endpoint can arrive from tracker, DHT and PEX. Go checks
        // connectingToPeerAddr before starting each attempt; reserve it once.
        let mut seen = HashSet::new();
        eligible.retain(|p| seen.insert(p.addr));
        eligible.sort_by_cached_key(|p| {
            let fails = fail_counts.get(&p.addr).copied().unwrap_or(0);
            (
                fails,
                std::cmp::Reverse((
                    p.trusted,
                    self.peer_priority(p.addr),
                    PEER_ADDRESS_HASH.hash_one(p.addr.to_string()),
                )),
            )
        });
        eligible.truncate(max);
        for p in &eligible {
            known.retain(|x| x.addr != p.addr);
            dialing.insert(p.addr);
        }
        eligible
    }

    /// Peers waiting to dial right now (not connected, not dialing, not in
    /// backoff). Used as `pendingPeers` for `reducedDialTimeout` so a pool
    /// full of cooling-down failures does not force min timeout on the few
    /// ready candidates (Go's peers.Len() only counts dialable reserve).
    pub(crate) fn dial_pending_ready(&self) -> usize {
        let handles = self.conn_handles.read();
        let dialing = self.dialing_peers.lock();
        let backoff = self.dial_backoff.lock();
        let now = Instant::now();
        let known = self.known_peers.read();
        let mut seen = HashSet::new();
        known
            .iter()
            .filter(|p| {
                seen.insert(p.addr)
                    && !handles.contains_key(&p.addr)
                    && !dialing.contains(&p.addr)
                    && backoff.get(&p.addr).is_none_or(|until| *until <= now)
            })
            .count()
    }

    pub(crate) fn dial_finished(&self, addr: SocketAddr, ok: bool) {
        self.dialing_peers.lock().remove(&addr);
        if ok {
            // Handshake success alone must NOT clear fail_count/backoff.
            // Bunny retain runs: dial_ok cleared the penalty, then unhelpful RST
            // re-armed only fail_count=1 (2s), so the same Chinese DHT contacts
            // redialed ~9×/60s. Clear only via `clear_dial_penalty` once the
            // peer delivers useful data (Go: PopMax + no requeue avoids this).
        } else {
            // Exponential backoff: 2s, 4s, 8s, … capped at 60s. Dialer re-queues
            // so we retry without waiting solely on rediscovery.
            let n = {
                let mut counts = self.dial_fail_counts.lock();
                let e = counts.entry(addr).or_insert(0);
                *e = e.saturating_add(1);
                *e
            };
            let shift = n.saturating_sub(1).min(5);
            let secs = (2u64 << shift).min(60);
            self.dial_backoff
                .lock()
                .insert(addr, Instant::now() + Duration::from_secs(secs));
        }
        self.bump_change();
    }

    /// Wipe dial fail/backoff after the peer proves helpful (useful chunk).
    pub(crate) fn clear_dial_penalty(&self, addr: SocketAddr) {
        self.dial_backoff.lock().remove(&addr);
        self.dial_fail_counts.lock().remove(&addr);
    }

    /// Post-handshake close that never delivered useful data must count like a
    /// dial failure for scheduling. Handshake `dial_finished(ok=true)` no longer
    /// clears fail/backoff; DHT rediscovery must still respect the escalating
    /// penalty or the same dead contacts burn half-open slots (Bunny: 9×/60s).
    /// Go avoids this by not requeueing after PopMax + slower recirculation.
    /// Only reset/decode/eof; helpful peers (useful bytes or LastHelpful) are
    /// left alone so normal disconnects do not poison dial forever.
    pub(crate) fn maybe_penalize_unhelpful_close(
        &self,
        addr: SocketAddr,
        reason: &str,
        useful_bytes: i64,
        had_last_helpful: bool,
    ) {
        let kind = crate::stats::failure_kind(reason);
        if !matches!(kind, "reset" | "decode" | "eof") {
            return;
        }
        if useful_bytes > 0 || had_last_helpful {
            return;
        }
        self.dial_finished(addr, false);
    }

    /// Whether a failed dial should be re-queued for another attempt.
    pub(crate) fn should_requeue_failed_dial(&self, addr: SocketAddr) -> bool {
        self.dial_fail_counts
            .lock()
            .get(&addr)
            .copied()
            .unwrap_or(0)
            < 8
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
    fn wants_conns_at_max_without_bad_conn_is_zero() {
        let t = torrent();
        t.max_established_conns.store(0, Ordering::Relaxed);
        assert_eq!(
            t.wants_conns(),
            0,
            "at max with no replaceable bad conn, Go wantOutgoingConns is false"
        );
        t.max_established_conns.store(3, Ordering::Relaxed);
        assert_eq!(t.wants_conns(), 3);
    }

    #[test]
    fn worst_bad_conn_none_when_empty() {
        let t = torrent();
        assert!(t.worst_bad_conn_addr(false, false).is_none());
        assert!(t.worst_conn_addr().is_none());
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
    fn dial_pool_prefers_fresh_peers_before_failed_then_trust_bep40() {
        let t = torrent();
        let local = "0.0.0.0:0".parse().unwrap();
        let mut peers: Vec<PeerInfo> = (1..=8).map(|n| PeerInfo {
            id: None, addr: format!("192.0.2.{n}:6881").parse().unwrap(),
            source: wt_types::PeerSource::Direct, supports_encryption: false,
            pex: Default::default(), trusted: true,
        }).collect();
        peers.sort_by_key(|p| std::cmp::Reverse(wt_discovery::bep40_priority(local, p.addr)));
        let best = peers[0].addr;
        let fresh = peers[1].addr;
        // Recirculated failures must not monopolize half-open over never-tried peers.
        t.dial_fail_counts.lock().insert(best, 7);
        t.add_peers(peers);
        let got: Vec<_> = t.take_dial_candidates(2).iter().map(|p| p.addr).collect();
        assert_eq!(got[0], fresh, "fresh peer before fail_count=7 contact: {got:?}");
        assert!(!got.contains(&best), "high fail_count deferred while fresh remain: {got:?}");
    }

    #[test]
    fn dial_pool_rediscovery_respects_fail_backoff_after_pop() {
        // Go PopMax: failed dials leave the pool; mid-run tracker/DHT/PEX
        // rediscovery re-adds. Backoff must still defer immediate redial so
        // rediscovery cannot burn half-open on the same dead contacts.
        let t = torrent();
        let peers: Vec<PeerInfo> = (1..=6)
            .map(|n| PeerInfo {
                id: None,
                addr: format!("198.51.100.{n}:6881").parse().unwrap(),
                source: PeerSource::DhtGetPeers,
                supports_encryption: false,
                pex: Default::default(),
                trusted: false,
            })
            .collect();
        t.add_peers(peers.clone());
        let batch = t.take_dial_candidates(6);
        assert_eq!(batch.len(), 6);
        assert_eq!(t.known_peers.read().len(), 0, "Pop removes from reserve");
        for p in &batch {
            t.dial_finished(p.addr, false);
            // Rediscovery (not dialer requeue) puts the contact back.
            t.add_peers(vec![p.clone()]);
        }
        assert_eq!(
            t.known_peers.read().len(),
            6,
            "rediscovery restores dial reserve after failures"
        );
        assert_eq!(
            t.take_dial_candidates(6).len(),
            0,
            "backoff must defer immediate re-dial of the same dead contacts"
        );
        assert_eq!(
            t.dial_pending_ready(),
            0,
            "reducedDialTimeout pending ignores backoff peers"
        );
        // Expire backoff and confirm peers are dialable again (not deleted forever).
        t.dial_backoff.lock().clear();
        let again = t.take_dial_candidates(6);
        assert_eq!(again.len(), 6, "after backoff, fail_counts must not delete forever");
        assert!(again.iter().all(|p| {
            t.dial_fail_counts.lock().get(&p.addr).copied().unwrap_or(0) == 1
        }));
    }

    #[test]
    fn unhelpful_reset_after_dial_ok_applies_backoff_and_defers_redial() {
        // dial_ok must NOT clear fail/backoff; unhelpful post-HS RST applies
        // dial_finished(false) so DHT re-add cannot burn half-open slots
        // (Bunny retain: same Chinese DHT contacts dial_ok→RST×9 when dial_ok
        // wiped fail_count back to 0 each time).
        let t = torrent();
        let addr: SocketAddr = "180.112.182.96:20081".parse().unwrap();
        let peer = PeerInfo {
            id: None,
            addr,
            source: PeerSource::DhtGetPeers,
            supports_encryption: false,
            pex: Default::default(),
            trusted: false,
        };
        t.add_peers(vec![peer.clone()]);
        assert_eq!(t.take_dial_candidates(1).len(), 1);
        t.dial_finished(addr, true);
        assert!(
            t.dial_fail_counts.lock().get(&addr).is_none(),
            "fresh dial_ok leaves fail_count empty"
        );

        t.maybe_penalize_unhelpful_close(
            addr,
            "write error: Connection reset by peer (os error 104)",
            0,
            false,
        );
        assert_eq!(
            t.dial_fail_counts.lock().get(&addr).copied().unwrap_or(0),
            1,
            "unhelpful reset must increment fail_count like dial_finished(false)"
        );
        assert!(
            t.dial_backoff.lock().get(&addr).is_some_and(|until| *until > Instant::now()),
            "unhelpful reset must arm exp backoff"
        );

        // DHT mid-run rediscovery re-adds; take_dial_candidates must still defer.
        t.add_peers(vec![peer]);
        assert_eq!(
            t.take_dial_candidates(4).len(),
            0,
            "backoff after unhelpful close must defer immediate redial"
        );
        assert_eq!(t.dial_pending_ready(), 0);
        assert!(t.should_requeue_failed_dial(addr));
    }

    #[test]
    fn dial_ok_does_not_reset_fail_count_so_unhelpful_churn_escalates() {
        // Regression for retain-1/2: dial_ok cleared fail_count → every RST
        // restarted at 2s backoff → 9 dials/60s on the same addr.
        let t = torrent();
        let addr: SocketAddr = "117.84.182.58:20027".parse().unwrap();
        let peer = PeerInfo {
            id: None,
            addr,
            source: PeerSource::DhtGetPeers,
            supports_encryption: false,
            pex: Default::default(),
            trusted: false,
        };
        for expected in 1u8..=4 {
            if expected > 1 {
                t.add_peers(vec![peer.clone()]);
                assert_eq!(
                    t.take_dial_candidates(1).len(),
                    0,
                    "fail {expected}: must still be in backoff from prior RST"
                );
                // Expire backoff so we can dial again (simulates time passing).
                t.dial_backoff.lock().remove(&addr);
            }
            t.add_peers(vec![peer.clone()]);
            assert_eq!(t.take_dial_candidates(1).len(), 1);
            t.dial_finished(addr, true);
            assert_eq!(
                t.dial_fail_counts.lock().get(&addr).copied().unwrap_or(0),
                expected.saturating_sub(1),
                "dial_ok must preserve prior fail_count (expected {})",
                expected.saturating_sub(1)
            );
            t.maybe_penalize_unhelpful_close(
                addr,
                "write error: Connection reset by peer (os error 104)",
                0,
                false,
            );
            assert_eq!(
                t.dial_fail_counts.lock().get(&addr).copied(),
                Some(expected),
                "each unhelpful close must escalate fail_count"
            );
        }
        // After 4 failures, backoff is 2<<3=16s — not the flat 2s loop.
        let until = *t.dial_backoff.lock().get(&addr).expect("backoff armed");
        assert!(
            until >= Instant::now() + Duration::from_secs(8),
            "escalated backoff must exceed first-step 2s, got {:?}",
            until.saturating_duration_since(Instant::now())
        );
        // Helpful chunk clears so a later rediscovery can dial fresh.
        t.clear_dial_penalty(addr);
        assert!(t.dial_fail_counts.lock().get(&addr).is_none());
        assert!(t.dial_backoff.lock().get(&addr).is_none());
        t.add_peers(vec![peer]);
        assert_eq!(t.take_dial_candidates(1).len(), 1);
    }

    #[test]
    fn unhelpful_decode_eof_kinds_also_penalize_zero_byte_closes() {
        let t = torrent();
        let decode_addr: SocketAddr = "221.228.157.207:20041".parse().unwrap();
        let eof_addr: SocketAddr = "198.51.100.9:6881".parse().unwrap();
        t.dial_finished(decode_addr, true);
        t.dial_finished(eof_addr, true);
        t.maybe_penalize_unhelpful_close(
            decode_addr,
            "frame decode error: Connection reset by peer (os error 104)",
            0,
            false,
        );
        t.maybe_penalize_unhelpful_close(
            eof_addr,
            "connection closed by peer (EOF)",
            0,
            false,
        );
        assert_eq!(t.dial_fail_counts.lock().get(&decode_addr).copied(), Some(1));
        assert_eq!(t.dial_fail_counts.lock().get(&eof_addr).copied(), Some(1));
        assert!(t.dial_backoff.lock().contains_key(&decode_addr));
        assert!(t.dial_backoff.lock().contains_key(&eof_addr));
    }

    #[test]
    fn helpful_peer_close_does_not_poison_dial_forever() {
        let t = torrent();
        let addr: SocketAddr = "203.0.113.77:51413".parse().unwrap();
        let peer = PeerInfo {
            id: None,
            addr,
            source: PeerSource::Tracker,
            supports_encryption: false,
            pex: Default::default(),
            trusted: false,
        };
        t.add_peers(vec![peer.clone()]);
        let _ = t.take_dial_candidates(1);
        t.dial_finished(addr, true);

        // Useful bytes → never treat as dial failure, even on reset/eof.
        t.maybe_penalize_unhelpful_close(
            addr,
            "write error: Connection reset by peer (os error 104)",
            16_384,
            true,
        );
        assert!(
            t.dial_fail_counts.lock().get(&addr).is_none(),
            "helpful peer must not gain fail_count"
        );
        assert!(
            t.dial_backoff.lock().get(&addr).is_none(),
            "helpful peer must not get dial backoff"
        );

        // LastHelpful alone (no bytes race) also protects.
        t.maybe_penalize_unhelpful_close(
            addr,
            "connection closed by peer (EOF)",
            0,
            true,
        );
        assert!(t.dial_fail_counts.lock().get(&addr).is_none());

        // Intentional choker drop must not use dial-fail path.
        t.maybe_penalize_unhelpful_close(addr, "choked too long", 0, false);
        assert!(t.dial_fail_counts.lock().get(&addr).is_none());

        t.add_peers(vec![peer]);
        assert_eq!(
            t.take_dial_candidates(1).len(),
            1,
            "after helpful close, rediscovery must dial immediately"
        );
    }

    #[test]
    fn peer_closed_path_applies_unhelpful_backoff_via_conn_handle() {
        use crate::peer_conn::PeerTorrent;
        use std::sync::atomic::AtomicBool;
        use tokio::sync::{mpsc, Notify};
        use wt_types::Transport;

        let t = torrent();
        let addr: SocketAddr = "180.112.182.96:20027".parse().unwrap();
        let peer = PeerInfo {
            id: None,
            addr,
            source: PeerSource::DhtGetPeers,
            supports_encryption: false,
            pex: Default::default(),
            trusted: false,
        };
        t.add_peers(vec![peer.clone()]);
        let _ = t.take_dial_candidates(1);
        t.dial_finished(addr, true);

        let (tx, _rx) = mpsc::unbounded_channel();
        let stats = Arc::new(crate::stats::AtomicConnStats::default());
        let handle = crate::conn::ConnHandle::new(
            addr,
            true,
            Transport::Tcp,
            false,
            t.info_hash,
            None,
            PeerSource::DhtGetPeers,
            [0u8; 8],
            tx,
            Arc::new(Notify::new()),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&stats),
        );
        handle.close("write error: Connection reset by peer (os error 104)");
        t.conn_handles.write().insert(addr, handle);

        PeerTorrent::peer_closed(&t, addr);

        assert_eq!(t.dial_fail_counts.lock().get(&addr).copied(), Some(1));
        assert!(t.dial_backoff.lock().contains_key(&addr));
        t.add_peers(vec![peer]);
        assert_eq!(
            t.take_dial_candidates(2).len(),
            0,
            "peer_closed unhelpful reset must defer DHT redial"
        );

        // Helpful path: useful bytes protect against poisoning.
        let addr2: SocketAddr = "203.0.113.88:6881".parse().unwrap();
        t.dial_finished(addr2, true);
        let (tx2, _rx2) = mpsc::unbounded_channel();
        let stats2 = Arc::new(crate::stats::AtomicConnStats::default());
        stats2
            .bytes_read_useful_data
            .store(32_768, Ordering::Relaxed);
        let handle2 = crate::conn::ConnHandle::new(
            addr2,
            true,
            Transport::Tcp,
            false,
            t.info_hash,
            None,
            PeerSource::Tracker,
            [0u8; 8],
            tx2,
            Arc::new(Notify::new()),
            Arc::new(AtomicBool::new(false)),
            stats2,
        );
        handle2.close("connection closed by peer (EOF)");
        t.conn_handles.write().insert(addr2, handle2);
        PeerTorrent::peer_closed(&t, addr2);
        assert!(
            t.dial_fail_counts.lock().get(&addr2).is_none(),
            "helpful peer_closed must not poison dial"
        );
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

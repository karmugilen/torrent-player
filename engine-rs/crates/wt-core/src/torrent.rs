//! Torrent state machine, piece management, and reader synchronization.

use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::LazyLock;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI32, AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use wt_types::{
    ClientStorage, Info, InfoHash, InfoHashV2, MetaInfo, PeerId, PeerInfo, PeerSource, PieceIndex,
    PiecePriority, RequestIndex, TorrentStorage, Transport, BLOCK_SIZE,
};

use crate::picker::piece_req_index_range;
use crate::sched::{BlockTable, PeerLive, SharedQueue};
use crate::reader::PieceRange;
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
    pub metadata: RwLock<crate::metadata::MetadataExchange>,
    pub pex: Mutex<crate::pex::PexState>,
    pub pex_last_recv: Mutex<HashMap<SocketAddr, Instant>>,
    pub peer_listen_ports: RwLock<HashMap<SocketAddr, u16>>,
    pub hasher: Mutex<Option<crate::hashing::PieceHasherPool>>,
    pub hash_seq: AtomicU64,
    pub dht_nodes: RwLock<Vec<SocketAddr>>,
    pub dirty_chunks: crate::piece_state::ChunkBits,
    /// Blocks on their way and being written.
    pub blocks: BlockTable,
    /// What refills read about every connected peer.
    pub(crate) live_peers: RwLock<Vec<Arc<PeerLive>>>,

    /// Peer key to address, so looking up one peer does not scan them all.
    pub peer_addrs: RwLock<HashMap<u64, SocketAddr>>,
    /// Peers announcing each piece, excluding have-all peers. Empty until info.
    pub(crate) piece_availability: std::sync::OnceLock<Box<[AtomicI32]>>,
    /// Have-all peers, counted once so they join and leave in O(1).
    pub(crate) have_all_peers: AtomicI32,
    /// Threads inside `change_cond`; `notify_progress` skips the lock without any.
    pub(crate) change_waiters: AtomicUsize,
    /// Pieces whose last hash failed, and a count of such failures.
    pub failed_pieces: Mutex<HashSet<PieceIndex>>,
    pub hash_fail_generation: AtomicU64,
    /// Connect results per peer source, to dial the likeliest sources first.
    pub(crate) dial_sources: Mutex<HashMap<wt_types::PeerSource, (u32, u32)>>,
    /// When a warm-up request last went out, in ms since `warm_epoch`.
    pub(crate) last_warm_ms: AtomicU64,
    /// When claims of playhead blocks last woke the swarm, same clock.
    pub(crate) last_seek_nudge_ms: AtomicU64,
    pub(crate) warm_epoch: Instant,
    /// Bumped under the `pieces` write lock; see [`Self::pieces_mut`].
    pieces_generation: AtomicU64,
    pub(crate) wanted_pieces: Mutex<Option<Arc<WantedPieces>>>,
    pub(crate) saved_queue: Mutex<SharedQueue>,
    /// Saved to the user's files, not Direct Play.
    pub saved_mode: std::sync::atomic::AtomicBool,
    pub(crate) last_refocus: Mutex<Option<Instant>>,
    pub(crate) refocus_seq: AtomicU64,
    pub(crate) last_read_nudge: Mutex<Option<Instant>>,
    pub known_peers: RwLock<HashSet<PeerInfo>>,
    pub client: RwLock<Weak<crate::client::ClientInner>>,
    pub dialing_peers: Mutex<HashSet<SocketAddr>>,
    /// Not-before instant for re-queued dial failures (exp backoff).
    pub dial_backoff: Mutex<HashMap<SocketAddr, Instant>>,
    /// Consecutive failed dials per addr; drives exponential re-queue backoff
    /// and dial-candidate ordering (fresh before recirculated dead contacts).
    pub swarm_diagnostics: Mutex<crate::stats::SwarmDiagnostics>,
    pub dial_fail_counts: Mutex<HashMap<SocketAddr, u8>>,
    /// Bumped when the network route changes; cancels dials on the old route.
    pub(crate) route_epoch: tokio::sync::watch::Sender<u64>,
    pub conn_handles: RwLock<HashMap<SocketAddr, crate::conn::ConnHandle>>,
    /// Lifetime totals survive connection churn, like Go torrent.ConnStats.
    pub(crate) retired_conn_stats: Mutex<crate::stats::ConnStats>,
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

    pub reader_ranges: RwLock<HashMap<u64, PieceRange>>,
    /// Peer start burst: the connection limit is raised until this instant.
    pub peer_burst_until: Mutex<Option<Instant>>,
    /// The next allowed download starts a peer burst.
    pub peer_burst_armed: AtomicBool,
    /// Connection limit while bursting; bursts share [`PEER_BURST_CONNS`]
    /// across all torrents.
    pub peer_burst_limit: AtomicUsize,
    /// Last playhead jump and how many of its eager hedges are left.
    pub seek_hedge: Mutex<Option<(Instant, u32)>>,
}

/// Connections and connection attempts allowed during a peer start burst,
/// shared by all torrents bursting at once.
pub const PEER_BURST_CONNS: usize = 250;

/// Pieces this rare come first. Above it, file order: blocks then land next to
/// each other on disk instead of all over a sparse file.
const RARE_AVAILABILITY: i32 = 2;

/// Rank among equal-priority pieces, lower first. Only `Normal` pieces have one:
/// anything higher is near a playhead and stays in file order. Started pieces
/// come first so hashing and write-back see whole pieces; rare ones are the most
/// likely to vanish and the most useful to trade.
fn background_rank(priority: PiecePriority, partial: bool, availability: i32) -> (bool, i32) {
    if priority == PiecePriority::Normal {
        (!partial, availability.min(RARE_AVAILABILITY + 1))
    } else {
        (false, 0)
    }
}

/// One scan of `pieces` shared by every peer's refill.
pub(crate) struct WantedPieces {
    pub(crate) generation: u64,
    /// Being hashed or written; they hold RAM but left `candidates`.
    pub(crate) checking: usize,
    built_at: Instant,
    pub(crate) num_pieces: u32,
    pub(crate) count: u32,
    /// Wanted and not complete; unlike `count`, includes pieces being hashed.
    pub(crate) incomplete: u32,
    pub(crate) requestable: Vec<bool>,
    /// `(index, priority, partial, chunks, availability)`, best first.
    pub(crate) candidates: Vec<(PieceIndex, PiecePriority, bool, usize, i32)>,
}
/// Availability moves with every have, so it refreshes on a timer instead of
/// invalidating [`WantedPieces`].
const WANTED_MAX_AGE: Duration = Duration::from_secs(1);
/// How long a peer start burst lasts before trimming to the configured limit.
pub const PEER_BURST_FOR: Duration = Duration::from_secs(10);
/// After a seek, the first blocks are hedged without waiting this long.
pub const SEEK_HEDGE_WINDOW: Duration = Duration::from_secs(2);
/// Least time between swarm wakes caused by peers claiming playhead blocks.
pub(crate) const SEEK_NUDGE_GAP_MS: u64 = 10;
/// Eager hedges per seek. The window alone followed the moving playhead and
/// duplicated every block read in it (measured: 37% of a seek run wasted).
pub const SEEK_HEDGES: u32 = 64;

impl TorrentInner {
    pub fn new(
        info_hash: InfoHash,
        info_hash_v2: Option<InfoHashV2>,
        display_name: String,
        default_storage: Arc<dyn ClientStorage>,
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
            dirty_chunks: Default::default(),
            blocks: BlockTable::default(),
            live_peers: RwLock::new(Vec::new()),
            peer_addrs: RwLock::new(HashMap::new()),
            piece_availability: Default::default(),
            have_all_peers: AtomicI32::new(0),
            change_waiters: AtomicUsize::new(0),
            failed_pieces: Default::default(),
            hash_fail_generation: AtomicU64::new(0),
            dial_sources: Default::default(),
            last_warm_ms: AtomicU64::new(0),
            last_seek_nudge_ms: AtomicU64::new(0),
            warm_epoch: Instant::now(),
            pieces_generation: AtomicU64::new(0),
            wanted_pieces: Mutex::new(None),
            saved_queue: Mutex::new(SharedQueue::default()),
            saved_mode: std::sync::atomic::AtomicBool::new(false),
            last_refocus: Mutex::new(None),
            refocus_seq: AtomicU64::new(0),
            last_read_nudge: Mutex::new(None),
            known_peers: RwLock::new(HashSet::new()),
            client: RwLock::new(Weak::new()),
            dialing_peers: Mutex::new(HashSet::new()),
            dial_backoff: Mutex::new(HashMap::new()),
            swarm_diagnostics: Mutex::new(Default::default()),
            dial_fail_counts: Mutex::new(HashMap::new()),
            route_epoch: tokio::sync::watch::channel(0).0,
            conn_handles: RwLock::new(HashMap::new()),
            retired_conn_stats: Mutex::new(crate::stats::ConnStats::default()),
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
            reader_ranges: RwLock::new(HashMap::new()),
            peer_burst_until: Mutex::new(None),
            peer_burst_armed: AtomicBool::new(true),
            peer_burst_limit: AtomicUsize::new(0),
            seek_hedge: Mutex::new(None),
        }
    }

    /// The only way to write `pieces`: invalidates [`WantedPieces`]. The bump
    /// happens with the lock held, so a reader never pairs new data with an
    /// old generation or the reverse.
    pub fn pieces_mut(&self) -> parking_lot::RwLockWriteGuard<'_, Vec<PieceEntry>> {
        let guard = self.pieces.write();
        self.pieces_generation.fetch_add(1, Ordering::Relaxed);
        guard
    }

    /// The per-piece refill inputs, rebuilt only when `pieces` changed. Every
    /// peer refills at once after a seek; all of them share one scan.
    pub(crate) fn wanted_pieces(&self) -> Arc<WantedPieces> {
        let pieces = self.pieces.read();
        let generation = self.pieces_generation.load(Ordering::Relaxed);
        let mut cached = self.wanted_pieces.lock();
        if let Some(wanted) = cached
            .as_ref()
            .filter(|w| w.generation == generation && w.built_at.elapsed() < WANTED_MAX_AGE)
        {
            return wanted.clone();
        }
        let mut requestable = Vec::with_capacity(pieces.len());
        let mut candidates = Vec::new();
        let mut count = 0u32;
        let mut incomplete = 0u32;
        let mut checking = 0usize;
        for p in pieces.iter() {
            checking += usize::from(p.checking);
            let wanted = !p.complete && !p.checking && p.priority > PiecePriority::None;
            count += u32::from(wanted);
            incomplete += u32::from(!p.complete && p.priority > PiecePriority::None);
            requestable.push(wanted);
            if wanted {
                candidates.push((p.index, p.priority, p.partial, p.chunks_total, self.availability(p.index)));
            }
        }
        // Pieces were collected in index order, so a stable sort keeps index
        // as the tie-break. Same order as `piece_order_cmp`.
        candidates.sort_by_key(|c| {
            (std::cmp::Reverse(c.1), background_rank(c.1, c.2, c.4))
        });
        let wanted = Arc::new(WantedPieces {
            generation,
            checking,
            built_at: Instant::now(),
            num_pieces: pieces.len() as u32,
            count,
            incomplete,
            requestable,
            candidates,
        });
        *cached = Some(wanted.clone());
        wanted
    }

    pub fn bump_change(&self) {
        self.peer_notify.notify_one();
        for handle in self.conn_handles.read().values() {
            handle.wake();
        }
        self.notify_progress();
    }

    /// Wake readers and status waiters only. Received data changes progress
    /// but creates no request work, so peers are not all rescheduled for
    /// every block (that cost grew with peers × incoming blocks).
    pub fn notify_progress(&self) {
        self.change_version.fetch_add(1, Ordering::SeqCst);
        // Waiters register before checking the version, so one of the two
        // SeqCst sides always sees the other.
        if self.change_waiters.load(Ordering::SeqCst) == 0 {
            return;
        }
        let _guard = self.change_mutex.lock();
        self.change_cond.notify_all();
    }

    /// Wait for `change_version` to pass `after`, up to `timeout`.
    pub fn wait_version(&self, after: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        let mut guard = self.change_mutex.lock();
        self.change_waiters.fetch_add(1, Ordering::SeqCst);
        let version = loop {
            let cur = self.change_version.load(Ordering::SeqCst);
            let wait = deadline.saturating_duration_since(Instant::now());
            if cur > after || wait.is_zero() {
                break cur;
            }
            self.change_cond.wait_for(&mut guard, wait);
        };
        self.change_waiters.fetch_sub(1, Ordering::SeqCst);
        version
    }

    /// Peers known to have `piece`.
    pub(crate) fn availability(&self, piece: PieceIndex) -> i32 {
        let announced = self
            .piece_availability
            .get()
            .and_then(|counts| counts.get(piece as usize))
            .map_or(0, |count| count.load(Ordering::Relaxed));
        announced + self.have_all_peers.load(Ordering::Relaxed)
    }

    /// Established-connection limit now: the configured one, raised while a
    /// start burst runs (see [`Self::update_peer_burst_limit`]).
    pub(crate) fn conn_limit(&self) -> usize {
        let max = self.max_established_conns.load(Ordering::Relaxed);
        if max > 0 && self.bursting() {
            max.max(self.peer_burst_limit.load(Ordering::Relaxed))
        } else {
            max
        }
    }

    /// Raise the burst limit by the connections the whole client may still
    /// open for bursts: several torrents resuming together must not open
    /// [`PEER_BURST_CONNS`] each. Called by the dialer without locks held.
    pub(crate) fn update_peer_burst_limit(&self, client_established: usize) {
        let mine = self.conn_handles.read().len();
        let room = PEER_BURST_CONNS.saturating_sub(client_established);
        self.peer_burst_limit.store((mine + room).min(PEER_BURST_CONNS), Ordering::Relaxed);
    }

    pub(crate) fn bursting(&self) -> bool {
        self.peer_burst_until.lock().is_some_and(|until| Instant::now() < until)
    }

    /// Start a peer burst: connect to many peers while a download starts,
    /// then keep the most productive ones (see [`Self::end_peer_burst_if_due`]).
    /// Skipped for resting sessions (limit 0) and complete torrents.
    pub fn start_peer_burst(&self) {
        let max = self.max_established_conns.load(Ordering::Relaxed);
        if max == 0 || max >= PEER_BURST_CONNS || self.closed.load(Ordering::Relaxed) {
            return;
        }
        let wanted = self.info.read().is_none()
            || self.pieces.read().iter().any(|p| !p.complete && p.priority > PiecePriority::None);
        if !wanted {
            return;
        }
        *self.peer_burst_until.lock() = Some(Instant::now() + PEER_BURST_FOR);
        // The dialer sets the shared limit on its next pass.
        self.peer_notify.notify_one();
    }

    /// When a burst ends, trim to the configured limit, dropping the peers
    /// that delivered the least useful data during it.
    pub(crate) fn end_peer_burst_if_due(&self) {
        {
            let mut until = self.peer_burst_until.lock();
            match *until {
                Some(at) if Instant::now() >= at => *until = None,
                _ => return,
            }
        }
        let max = self.max_established_conns.load(Ordering::Relaxed);
        let ranks = self.peer_ranks();
        let snapshots = self.peer_snapshots.read();
        let mut peers: Vec<(SocketAddr, f64, bool, bool, i64)> = self.conn_handles.read().iter()
            .filter(|(_, h)| !h.is_closed())
            .map(|(addr, h)| {
                let snap = snapshots.get(addr);
                (*addr, snap.map_or(0.0, |s| s.download_rate), snap.is_some_and(|s| !s.peer_choking),
                    ranks.get(addr).is_some_and(|r| r.0),
                    h.stats().bytes_read_useful_data.load(Ordering::Relaxed))
            })
            .collect();
        drop(snapshots);
        if peers.len() <= max {
            return;
        }
        // Choked peers first: they send nothing now. Then slowest by current
        // rate, not total bytes: a peer that only just unchoked us has few
        // bytes but may be the fastest here.
        peers.sort_by(|a, b| a.2.cmp(&b.2)
            .then(a.1.total_cmp(&b.1)).then(a.3.cmp(&b.3)).then(a.4.cmp(&b.4)));
        let excess = peers.len() - max;
        for (addr, ..) in peers.into_iter().take(excess) {
            self.drop_conn_addr(addr, "peer burst ended");
        }
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
            self.peer_notify.notify_one();
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
        // One slot per peer: reject a second connection by address or peer ID
        // (a peer can reach us inbound and outbound from different ports).
        let peer_id = info.peer_id;
        let has_id = peer_id.iter().any(|b| *b != 0);
        let duplicate = |handles: &HashMap<SocketAddr, crate::conn::ConnHandle>| {
            handles.contains_key(&info.remote)
                || (has_id && handles.values().any(|h| !h.is_closed() && h.peer_id() == Some(peer_id)))
        };
        if duplicate(&self.conn_handles.read()) {
            return Err(CoreError::Other("duplicate peer".into()));
        }
        let max = self.conn_limit();
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
        if duplicate(&handles) {
            return Err(CoreError::Other("duplicate peer".into()));
        }
        if handles.len() >= max {
            // Race: another attach won the slot.
            return Err(CoreError::Other("don't want conn".into()));
        }
        handles.insert(info.remote, handle.clone());
        drop(handles);
        self.peer_addrs.write().insert(crate::torrent_impls::peer_key_from_addr(info.remote), info.remote);
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
        if valid {
            self.failed_pieces.lock().remove(&piece);
        } else {
            let _ = ps.mark_not_complete();
            self.failed_pieces.lock().insert(piece);
            self.hash_fail_generation.fetch_add(1, Ordering::Relaxed);
            wt_types::events::record("hash_fail", format!("piece={piece}"));
        }
        {
            let mut pieces = self.pieces_mut();
            if let Some(entry) = pieces.get_mut(piece as usize) {
                entry.checking = false;
                entry.complete = valid;
                entry.ok = true;
                if !valid {
                    entry.partial = false;
                }
            }
        }
        if !valid {
            let (begin, end) = piece_req_index_range(piece, info.piece_length, info.total_length());
            self.dirty_chunks.remove_range(begin, end);
        }
        if valid {
            for handle in self.conn_handles.read().values() {
                handle.send_message(wt_wire::message::Message::Have { index: piece });
            }
        }
        // A finished piece frees no blocks, so a saved download need not wake
        // every peer to refill at once. Failures and playback still do.
        if valid && crate::peer_conn::PeerTorrent::background_only(self) && self.wanted_pieces().count > 0 {
            self.peer_notify.notify_one();
            self.notify_progress();
        } else {
            self.bump_change();
        }
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
        let maximum = self.conn_limit() as i64;
        let handles = self.conn_handles.read();
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
        let max = self.conn_limit();
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

    /// Go `PeerConn.useful()` and `lastHelpful()` for each live connection.
    /// Ranking through that
    /// map alone made every peer look useless and never helpful, so the
    /// longest-connected (usually most productive) peers were replaced first.
    /// Locks are taken one at a time, never nested.
    fn peer_ranks(&self) -> HashMap<SocketAddr, (bool, Option<Instant>)> {
        let now = Instant::now();
        let has_info = self.info.read().is_some();
        let seeding = self.seeding.load(Ordering::Relaxed);
        let wanted: Vec<bool> = self.pieces.read().iter()
            .map(|p| p.priority > PiecePriority::None && !p.complete).collect();
        let snaps: HashMap<SocketAddr, (Option<Duration>, bool)> = self.peer_snapshots.read()
            .iter().map(|(a, s)| (*a, (s.last_useful_data, s.peer_interested))).collect();
        let live: Vec<(SocketAddr, [u8; 8])> = self.conn_handles.read().iter()
            .filter(|(_, h)| !h.is_closed())
            .map(|(a, h)| (*a, h.peer_extension_bits())).collect();
        let mut ranks = HashMap::with_capacity(live.len());
        for (addr, bits) in live {
            let (since_useful, interested) = snaps.get(&addr).copied().unwrap_or((None, false));
            let last = since_useful.and_then(|d| now.checked_sub(d));
            let useful = if !has_info {
                wt_wire::handshake::PeerExtensionBits(bits).supports_extended()
            } else if seeding && interested {
                true
            } else {
                let key = crate::torrent_impls::peer_key_from_addr(addr);
                self.live_peers.read().iter().find(|p| p.key == key)
                    .is_some_and(|p| p.pieces.read().iter().any(|i| wanted.get(i as usize).copied().unwrap_or(false)))
            };
            ranks.insert(addr, (useful, last));
        }
        ranks
    }

    /// Go `worstBadConn` / `worstBadConnFromSlice`: pick a replaceable bad
    /// connection (bad direction, wasted chunks, or aged worse-half).
    pub(crate) fn worst_bad_conn_addr(
        &self,
        incoming_is_bad: bool,
        outgoing_is_bad: bool,
    ) -> Option<SocketAddr> {
        let ranks = self.peer_ranks();
        let handles = self.conn_handles.read();
        if handles.is_empty() {
            return None;
        }
        let max = self.conn_limit();

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
            let (useful, last_helpful) = ranks.get(addr).copied().unwrap_or((false, None));
            cands.push(Cand {
                addr: *addr,
                bad_direction,
                useful,
                last_helpful,
                completed_handshake: handle.connected_at(),
                wasted: stats.chunks_read_wasted,
                useful_chunks: stats.chunks_read_useful,
                peer_prio: self.peer_priority(*addr),
            });
        }
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
        let ranks = self.peer_ranks();
        let handles = self.conn_handles.read();
        if handles.is_empty() {
            return None;
        }
        let mut best: Option<(SocketAddr, bool, bool, Option<Instant>, Instant, u32)> = None;
        for (addr, handle) in handles.iter() {
            if handle.is_closed() {
                continue;
            }
            let (useful, last_helpful) = ranks.get(addr).copied().unwrap_or((false, None));
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
        if let Some(handle) = self.retire_connection(addr) {
            handle.close(reason);
            *self
                .swarm_diagnostics
                .lock()
                .closes
                .entry(crate::stats::failure_kind(reason).into())
                .or_default() += 1;
            #[cfg(not(target_os = "android"))]
            eprintln!(
                "peer_closed addr={} out={} src={:?} reason={}",
                addr,
                handle.is_outgoing(),
                handle.source(),
                reason.chars().take(120).collect::<String>()
            );
        }
        self.peer_snapshots.write().remove(&addr);
        self.forget_peer_playback_state(addr);
        self.choked_since.write().remove(&addr);
        self.bump_change();
    }

    pub(crate) fn forget_peer_playback_state(&self, addr: SocketAddr) {
        self.blocks.forget_peer(crate::torrent_impls::peer_key_from_addr(addr));
    }

    pub(crate) fn retire_connection(&self, addr: SocketAddr) -> Option<crate::conn::ConnHandle> {
        // Transfer counters under the same registry lock used by stats(), so
        // a snapshot counts a connection exactly once, whether live or closed.
        let mut handles = self.conn_handles.write();
        let handle = handles.remove(&addr)?;
        self.peer_addrs.write().remove(&crate::torrent_impls::peer_key_from_addr(addr));
        self.retired_conn_stats.lock().add(&handle.stats().snapshot());
        Some(handle)
    }

    pub(crate) fn note_dial_source(&self, source: wt_types::PeerSource, ok: bool) {
        let mut sources = self.dial_sources.lock();
        let entry = sources.entry(source).or_default();
        if ok {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    pub(crate) fn take_dial_candidates(&self, max: usize) -> Vec<PeerInfo> {
        if max == 0 {
            return Vec::new();
        }
        // Go prioritizedPeers.PopMax is trust then BEP 40. Pure Pop without
        // rediscovery drains the pool faster than tracker/DHT refill on Ubuntu
        // (pool 220→<40, dialFailures.timeout≈400). Failed dials are re-queued
        // with exp backoff; among ready peers prefer lower fail_counts so
        // recirculated dead contacts do not starve never-tried addrs.
        let now = Instant::now();
        let mut eligible: Vec<PeerInfo> = {
            let handles = self.conn_handles.read();
            let dialing = self.dialing_peers.lock();
            let backoff = self.dial_backoff.lock();
            let known = self.known_peers.read();
            let mut seen = HashSet::new();
            // The same endpoint can arrive from tracker, DHT and PEX. Go checks
            // connectingToPeerAddr before starting each attempt; reserve it once.
            known
                .iter()
                .filter(|p| {
                    !handles.contains_key(&p.addr)
                        && !dialing.contains(&p.addr)
                        && backoff.get(&p.addr).is_none_or(|until| *until <= now)
                        && seen.insert(p.addr)
                })
                .cloned()
                .collect()
        };
        // Sort with no peer table locked: the pool can hold hundreds.
        let fail_counts = self.dial_fail_counts.lock().clone();
        // Smoothed connect rate per thousand, so an untried source starts even.
        let source_rates: HashMap<_, _> = self.dial_sources.lock().iter()
            .map(|(&source, &(ok, failed))| (source, (ok + 1) * 1000 / (ok + failed + 2)))
            .collect();
        eligible.sort_by_cached_key(|p| {
            let fails = fail_counts.get(&p.addr).copied().unwrap_or(0);
            (
                fails,
                std::cmp::Reverse((
                    p.trusted,
                    source_rates.get(&p.source).copied().unwrap_or(500),
                    self.peer_priority(p.addr),
                    PEER_ADDRESS_HASH.hash_one(p.addr),
                )),
            )
        });
        eligible.truncate(max);
        let taken: HashSet<SocketAddr> = eligible.iter().map(|p| p.addr).collect();
        self.known_peers.write().retain(|p| !taken.contains(&p.addr));
        self.dialing_peers.lock().extend(taken);
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

    /// The network route changed: stop dials still running on the old route,
    /// which would hold half-open slots until they time out, and forget the
    /// backoff from dials that failed because there was no route.
    pub(crate) fn route_changed(&self) {
        self.route_epoch.send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        self.dial_backoff.lock().clear();
        self.dial_fail_counts.lock().clear();
        self.peer_notify.notify_one();
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
        // Only the dialer cares; waking every peer per failed dial is a herd.
        self.peer_notify.notify_one();
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

    /// Fail-count still below the old requeue cap (tests / diagnostics).
    #[cfg(test)]
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
        *self.pieces_mut() = piece_entries;
        let blocks = crate::picker::piece_req_index_range(0, piece_len, piece_len).1
            .saturating_mul(info.num_pieces());
        self.dirty_chunks.init(blocks);
        self.blocks.init(blocks);
        crate::torrent_impls::init_availability(self, info.num_pieces());
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


    /// Go `NudgeUrgentRequests`: wake every peer and have those with spare
    /// slots recompute requests now rather than when their queue runs low,
    /// which is also where stuck playhead blocks get rescued and hedged.
    /// Returns how many peers were asked.
    pub fn nudge_urgent_requests(&self) -> usize {
        if self.closed.load(Ordering::Relaxed)
            || self.data_download_disallowed.load(Ordering::Relaxed)
            || self.info.read().is_none()
        {
            return 0;
        }
        let urgent = self.pieces.read().iter()
            .any(|p| !p.complete && p.priority >= PiecePriority::Now);
        let handles = self.conn_handles.read();
        for handle in handles.values() {
            if urgent {
                handle.request_update();
            } else {
                handle.wake();
            }
        }
        if urgent { handles.len() } else { 0 }
    }

    /// Go `NudgeUrgentRequests` (Claude seek); see [`Self::nudge_urgent_requests`].
    pub fn nudge_urgent_peers(&self) {
        self.nudge_urgent_requests();
    }
}

#[cfg(test)]
mod dial_parity_tests {
    use super::*;

    fn torrent() -> TorrentInner {
        TorrentInner::new(InfoHash([7; 20]), None, "dial-parity".into(),
            Arc::new(crate::storage_adapter::MemoryClientStorage::new()))
    }

    #[test]
    fn peer_burst_shares_its_connections_across_torrents() {
        let t = torrent();
        t.max_established_conns.store(50, Ordering::Relaxed);
        assert_eq!(t.conn_limit(), 50);
        t.start_peer_burst();
        assert!(t.bursting());
        t.update_peer_burst_limit(0);
        assert_eq!(t.conn_limit(), PEER_BURST_CONNS);
        // Other torrents already hold most of the burst connections.
        t.update_peer_burst_limit(240);
        assert_eq!(t.conn_limit(), 50, "never below the configured limit");
        // A limit at or above the burst size, or a resting session, never bursts.
        let big = torrent();
        big.max_established_conns.store(PEER_BURST_CONNS, Ordering::Relaxed);
        big.start_peer_burst();
        assert!(!big.bursting());
        let resting = torrent();
        resting.max_established_conns.store(0, Ordering::Relaxed);
        resting.start_peer_burst();
        assert!(!resting.bursting());
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

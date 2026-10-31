//! Block scheduler: who holds which block, and what each peer fetches next.
//!
//! One table and one picker serve playback and saved downloads. A refill walks
//! the playhead window first, then a queue all peers share, then the end of
//! the selection and stalled blocks. Slow holders lose blocks in only two ways,
//! both here: an overdue playhead block gets a second copy or moves to a
//! quicker peer, and an idle peer takes a block stuck for [`STALLED_AFTER`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wt_types::{PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};

use crate::peer_conn::PeerKey;
use crate::piece_state::ChunkBits;
use crate::picker::{block_coord_from_req_index, piece_req_index_range};
use crate::torrent::TorrentInner;

/// Blocks ahead of the playhead the playhead pass looks after (2 MiB).
pub const PLAYHEAD_WINDOW_BLOCKS: u32 = 128;
/// Blocks at the playhead that go to the quickest peer, not the first to ask.
const FOCUS_BLOCKS: u32 = 8;
/// A peer leaves a focus block to one this many times quicker.
const QUICKER_RATIO: f64 = 1.5;
/// Blocks at a new playhead copied at once after a seek (1 MiB: what a player
/// needs for its first frame).
const SEEK_COPY_BLOCKS: u32 = 64;
/// A playhead block is overdue after this, or two holder round trips if longer.
const OVERDUE_AFTER: Duration = Duration::from_millis(150);
/// Unmeasured round trip when estimating arrivals.
const DEFAULT_RTT: f64 = 0.2;
/// An idle peer takes a block held this long from a holder under half its rate.
/// Queues hold 3 s of rate and mobile round trips reach 2.4 s.
const STALLED_AFTER: Duration = Duration::from_secs(8);
/// Pieces and blocks one stalled-block scan may look at and take.
const STALLED_SCAN_PIECES: usize = 256;
const STALLED_TAKE: usize = 16;
/// Below this rate a peer leaves `Next` pieces to faster peers that can finish
/// them sooner.
const SLOW_PEER_BYTES_PER_SEC: f64 = (3 * BLOCK_SIZE) as f64;
const SLOW_PEER_YIELDS: u32 = 10;
/// The last of the selection takes second copies so one peer cannot hold it up.
pub(crate) const ENDGAME_BYTES: i64 = 16 * 1024 * 1024;
/// RAM pieces may hold while unverified: playing, and saving with nothing
/// playing. Saving must stay under the storage's RAM so pieces finish there.
const PLAYBACK_OPEN_BYTES: i64 = 64 << 20;
const SAVED_OPEN_BYTES: i64 = 192 << 20;
const OPEN_MIN_PIECES: i64 = 4;
/// How soon a block handed back by a choke or a dropped peer is seen by the queue.
const REWIND_EVERY: Duration = Duration::from_millis(250);
/// Seconds of a peer's own rate kept requested outside playback. A longer queue
/// parks blocks where faster peers cannot fetch them.
const QUEUE_SECONDS: f64 = 3.0;
const QUEUE_FLOOR: usize = 32;
/// Limit until a peer has delivered anything: its speed is still unknown.
const UNMEASURED_QUEUE: usize = 32;
/// Round trips of a peer's rate kept requested during playback.
const PLAYBACK_QUEUE_RTTS: f64 = 2.0;
const PLAYBACK_QUEUE_MARGIN: usize = 2;
const SHARDS: usize = 32;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Request limit for one peer: bandwidth-delay during playback, otherwise a few
/// seconds of what it delivers, and never under two round trips: on mobile data
/// a shorter queue shrinks the rate it is sized from, and so on down.
pub fn peer_queue_limit(rate: f64, rtt: Duration, playing: bool) -> usize {
    if rate <= 0.0 || (playing && rtt.is_zero()) {
        return UNMEASURED_QUEUE;
    }
    if playing {
        let blocks = rate * rtt.as_secs_f64() * PLAYBACK_QUEUE_RTTS / f64::from(BLOCK_SIZE);
        return blocks as usize + PLAYBACK_QUEUE_MARGIN;
    }
    let seconds = QUEUE_SECONDS.max(rtt.as_secs_f64() * 2.0);
    ((rate * seconds / f64::from(BLOCK_SIZE)) as usize).max(QUEUE_FLOOR)
}

/// Seconds until a new request to this peer should arrive. Unmeasured peers are
/// last so that a peer with a known speed wins the playhead.
pub fn expected_arrival(rate: f64, rtt: Duration, queued: u32) -> f64 {
    if rate <= 0.0 {
        return f64::MAX;
    }
    let rtt = if rtt.is_zero() { DEFAULT_RTT } else { rtt.as_secs_f64() };
    rtt + f64::from(queued + 1) * f64::from(BLOCK_SIZE) / rate
}

// ---------------------------------------------------------------- Block table

#[derive(Clone, Copy, Default)]
struct Hold {
    owner: Option<(PeerKey, Instant)>,
    second: Option<PeerKey>,
}

/// Blocks on their way: their owner, when it was asked, and any second copy.
/// Only blocks in flight take memory, so a huge torrent costs nothing extra.
pub struct BlockTable {
    shards: Box<[Mutex<HashMap<RequestIndex, Hold>>]>,
    /// Blocks with an owner, read without a lock when scanning for free blocks.
    owned: ChunkBits,
    /// Blocks being written. Not readable and not requestable.
    writing: ChunkBits,
}

impl Default for BlockTable {
    fn default() -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Mutex::default()).collect(),
            owned: ChunkBits::new(),
            writing: ChunkBits::new(),
        }
    }
}

impl BlockTable {
    pub fn init(&self, blocks: u32) {
        self.owned.init(blocks);
        self.writing.init(blocks);
    }

    fn shard(&self, req: RequestIndex) -> &Mutex<HashMap<RequestIndex, Hold>> {
        &self.shards[req as usize % SHARDS]
    }

    #[inline]
    pub fn is_owned(&self, req: RequestIndex) -> bool {
        self.owned.contains(req)
    }

    #[inline]
    pub fn is_writing(&self, req: RequestIndex) -> bool {
        self.writing.contains(req)
    }

    /// True for the one caller allowed to write this block.
    pub fn begin_write(&self, req: RequestIndex) -> bool {
        self.writing.insert_new(req)
    }

    pub fn end_write(&self, req: RequestIndex) {
        self.writing.remove(req);
    }

    fn get(&self, req: RequestIndex) -> Hold {
        self.shard(req).lock().get(&req).copied().unwrap_or_default()
    }

    pub fn owner(&self, req: RequestIndex) -> Option<(PeerKey, Instant)> {
        if !self.is_owned(req) {
            return None;
        }
        self.get(req).owner
    }

    pub fn second(&self, req: RequestIndex) -> Option<PeerKey> {
        self.get(req).second
    }

    /// Becomes the owner unless another peer is.
    pub fn claim(&self, req: RequestIndex, peer: PeerKey, when: Instant) -> bool {
        let mut shard = self.shard(req).lock();
        let hold = shard.entry(req).or_default();
        if hold.owner.is_some_and(|(owner, _)| owner != peer) {
            return false;
        }
        hold.owner = Some((peer, when));
        self.owned.insert(req);
        true
    }

    pub fn release(&self, req: RequestIndex, peer: PeerKey) {
        let mut shard = self.shard(req).lock();
        if let Some(hold) = shard.get_mut(&req) {
            if hold.owner.is_some_and(|(owner, _)| owner == peer) {
                hold.owner = None;
                self.owned.remove(req);
                if hold.second.is_none() {
                    shard.remove(&req);
                }
            }
        }
    }

    pub fn clear_owner(&self, req: RequestIndex) {
        let mut shard = self.shard(req).lock();
        if let Some(hold) = shard.get_mut(&req) {
            hold.owner = None;
            self.owned.remove(req);
            if hold.second.is_none() {
                shard.remove(&req);
            }
        }
    }

    /// One second copy per block.
    pub fn claim_second(&self, req: RequestIndex, peer: PeerKey) -> bool {
        let mut shard = self.shard(req).lock();
        let hold = shard.entry(req).or_default();
        if hold.second.is_some() {
            return false;
        }
        hold.second = Some(peer);
        true
    }

    pub fn clear_second(&self, req: RequestIndex) {
        let mut shard = self.shard(req).lock();
        if let Some(hold) = shard.get_mut(&req) {
            hold.second = None;
            if hold.owner.is_none() {
                shard.remove(&req);
            }
        }
    }

    pub fn release_second(&self, req: RequestIndex, peer: PeerKey) {
        let mut shard = self.shard(req).lock();
        if let Some(hold) = shard.get_mut(&req) {
            if hold.second == Some(peer) {
                hold.second = None;
                if hold.owner.is_none() {
                    shard.remove(&req);
                }
            }
        }
    }

    /// The block is done: nobody holds it now.
    pub fn clear(&self, req: RequestIndex) {
        self.shard(req).lock().remove(&req);
        self.owned.remove(req);
    }

    /// Whether `peer` still owns or copies this block.
    pub fn holds(&self, req: RequestIndex, peer: PeerKey) -> bool {
        let hold = self.get(req);
        hold.second == Some(peer) || hold.owner.is_some_and(|(owner, _)| owner == peer)
    }

    /// A peer left: hand back everything it held.
    pub fn forget_peer(&self, peer: PeerKey) {
        for shard in self.shards.iter() {
            shard.lock().retain(|&req, hold| {
                if hold.second == Some(peer) {
                    hold.second = None;
                }
                if hold.owner.is_some_and(|(owner, _)| owner == peer) {
                    hold.owner = None;
                    self.owned.remove(req);
                }
                hold.owner.is_some() || hold.second.is_some()
            });
        }
    }
}

// ---------------------------------------------------------------- Peer board

/// What other peers' refills read about a peer, without a torrent-wide lock.
pub struct PeerLive {
    pub key: PeerKey,
    rate_bits: AtomicU64,
    rtt_micros: AtomicU64,
    queued: AtomicU32,
    nominal: AtomicU32,
    choking: AtomicBool,
    have_all: AtomicBool,
    /// Milliseconds since [`EPOCH`] plus one; 0 means never.
    last_useful: AtomicU64,
}

impl PeerLive {
    pub fn new(key: PeerKey) -> Self {
        Self {
            key,
            rate_bits: AtomicU64::new(0),
            rtt_micros: AtomicU64::new(0),
            queued: AtomicU32::new(0),
            nominal: AtomicU32::new(1),
            choking: AtomicBool::new(true),
            have_all: AtomicBool::new(false),
            last_useful: AtomicU64::new(0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn publish(&self, rate: f64, rtt: Duration, queued: usize, nominal: usize, choking: bool,
        have_all: bool, last_useful: Option<Instant>) {
        self.rate_bits.store(rate.to_bits(), Ordering::Relaxed);
        self.rtt_micros.store(rtt.as_micros() as u64, Ordering::Relaxed);
        self.queued.store(queued as u32, Ordering::Relaxed);
        self.nominal.store(nominal as u32, Ordering::Relaxed);
        self.choking.store(choking, Ordering::Relaxed);
        self.have_all.store(have_all, Ordering::Relaxed);
        self.last_useful.store(
            last_useful.map_or(0, |at| at.saturating_duration_since(*EPOCH).as_millis() as u64 + 1),
            Ordering::Relaxed,
        );
    }

    pub fn rate(&self) -> f64 {
        f64::from_bits(self.rate_bits.load(Ordering::Relaxed))
    }

    pub fn rtt(&self) -> Duration {
        Duration::from_micros(self.rtt_micros.load(Ordering::Relaxed))
    }

    fn last_useful_ms(&self) -> u64 {
        self.last_useful.load(Ordering::Relaxed)
    }

    pub fn arrival(&self) -> f64 {
        expected_arrival(self.rate(), self.rtt(), self.queued.load(Ordering::Relaxed))
    }

    /// Could take a request right now.
    fn has_room(&self) -> bool {
        !self.choking.load(Ordering::Relaxed)
            && self.queued.load(Ordering::Relaxed) < self.nominal.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------- Picker

/// What the picker needs to know about the refilling peer.
pub struct PickPeer<'a> {
    pub key: PeerKey,
    pub have_all: bool,
    pub pieces: &'a HashSet<PieceIndex>,
    pub allowed_fast: &'a HashSet<PieceIndex>,
    pub choking: bool,
    pub outstanding: &'a HashSet<RequestIndex>,
    pub cancelled: &'a HashSet<RequestIndex>,
    /// About to run dry: may start a piece even when RAM is spoken for.
    pub starving: bool,
    pub rate: f64,
    pub rtt: Duration,
    pub last_useful: Option<Instant>,
}

impl PickPeer<'_> {
    fn has(&self, piece: PieceIndex) -> bool {
        self.have_all || self.pieces.contains(&piece)
    }

    fn blocked(&self, piece: PieceIndex) -> bool {
        self.choking && !self.allowed_fast.contains(&piece)
    }

    fn arrival(&self) -> f64 {
        expected_arrival(self.rate, self.rtt, self.outstanding.len() as u32)
    }

    fn last_useful_ms(&self) -> u64 {
        self.last_useful
            .map_or(0, |at| at.saturating_duration_since(*EPOCH).as_millis() as u64 + 1)
    }
}

/// How a peer gets a picked block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Take {
    /// Nobody holds it.
    Free,
    /// A second copy; the first to arrive wins. `eager` copies are sent before
    /// the block is overdue and spend the seek's budget.
    Copy { eager: bool },
    /// The holder is stuck and loses it.
    From(PeerKey),
}

#[derive(Default)]
pub struct Pick {
    pub blocks: Vec<(RequestIndex, Take)>,
    /// The peer has something we want.
    pub interested: bool,
}

/// The block queue all peers share. A refill continues where the last one
/// stopped instead of re-reading every block already taken. Blocks handed back
/// are found at the next rewind.
#[derive(Default)]
pub(crate) struct SharedQueue {
    /// Per piece: blocks from its start already handed out or finished.
    next: Vec<u32>,
    rewound_at: Option<Instant>,
    /// Pieces holding RAM: started, or waiting on the hash and the disk.
    open: usize,
    generation: u64,
}

impl SharedQueue {
    pub(crate) fn rewind(&mut self) {
        self.rewound_at = None;
    }
}

impl TorrentInner {
    /// The next blocks `peer` should fetch, best first, at most `want`. Nothing
    /// is claimed here: the caller claims each block as it sends the request.
    pub(crate) fn pick_blocks(&self, peer: &PickPeer<'_>, want: usize) -> Pick {
        let mut pick = Pick::default();
        let Some(info) = self.info.read().clone() else {
            return pick;
        };
        let (plen, total) = (info.piece_length, info.total_length());
        let wanted = self.wanted_pieces();
        let saving = self.saved_mode.load(Ordering::Relaxed)
            && wanted.candidates.first().is_none_or(|c| c.1 < PiecePriority::Readahead);
        let focus = if saving { None } else { self.request_focus_index() };
        let window = focus.map(|f| {
            f..f.saturating_add(PLAYHEAD_WINDOW_BLOCKS).min(self.max_end_request())
        });
        if let Some(window) = &window {
            self.pick_playhead(peer, want, window.clone(), &wanted, plen, total, &mut pick);
        }
        let wanted_bytes = i64::from(wanted.count).saturating_mul(plen);
        if focus.is_none() && wanted_bytes <= ENDGAME_BYTES {
            self.pick_endgame(peer, want, &wanted, plen, total, &mut pick);
        } else {
            let open_bytes = if saving { SAVED_OPEN_BYTES } else { PLAYBACK_OPEN_BYTES };
            let open_limit = (open_bytes / plen.max(1)).max(OPEN_MIN_PIECES) as usize;
            self.pick_queue(peer, want, window, open_limit, &wanted, plen, total, &mut pick);
        }
        if pick.blocks.is_empty() && want > 0 && !peer.choking && peer.rate > 0.0 {
            self.pick_stalled(peer, want.min(STALLED_TAKE), &wanted, plen, total, &mut pick);
        }
        pick
    }

    fn block_free(&self, req: RequestIndex) -> bool {
        !self.dirty_chunks.contains(req) && !self.blocks.is_writing(req) && !self.blocks.is_owned(req)
    }

    /// Blocks the player needs next: the quickest peer gets them, and any that
    /// run late get a second copy or move.
    #[allow(clippy::too_many_arguments)]
    fn pick_playhead(&self, peer: &PickPeer<'_>, want: usize, window: std::ops::Range<u32>,
        wanted: &crate::torrent::WantedPieces, plen: i64, total: i64, pick: &mut Pick) {
        let focus = window.start;
        let steal_after = self.client.read().upgrade()
            .map_or(Duration::ZERO, |c| *c.urgent_steal_after.read());
        let tuning = crate::tuning::current_seek_tuning();
        let eager = self.seek_hedge.lock()
            .is_some_and(|(at, left)| left > 0 && at.elapsed() < crate::torrent::SEEK_HEDGE_WINDOW);
        let peers = self.live_peers.read();
        let bitfields = self.peer_pieces.read();
        let pieces = self.pieces.read();
        let mine = peer.arrival();
        let can_serve = |other: &PeerLive, piece: PieceIndex| {
            other.has_room()
                && (other.have_all.load(Ordering::Relaxed)
                    || bitfields.get(&other.key).is_some_and(|b| b.contains(&piece)))
        };
        let now = Instant::now();
        for req in window {
            if pick.blocks.len() >= want {
                break;
            }
            let piece = block_coord_from_req_index(req, plen, total).piece;
            if !wanted.requestable.get(piece as usize).copied().unwrap_or(false)
                || self.dirty_chunks.contains(req)
                || self.blocks.is_writing(req)
                || peer.outstanding.contains(&req)
                || peer.cancelled.contains(&req)
                || !peer.has(piece)
            {
                continue;
            }
            pick.interested = true;
            if peer.blocked(piece) {
                continue;
            }
            let holder = self.blocks.owner(req).filter(|&(owner, _)| owner != peer.key);
            let Some((holder, asked)) = holder else {
                let leave_it = req < focus + FOCUS_BLOCKS
                    && !steal_after.is_zero()
                    && peers.iter().any(|other| other.key != peer.key
                        && can_serve(other, piece)
                        && other.arrival() < mine / QUICKER_RATIO);
                if !leave_it {
                    pick.blocks.push((req, Take::Free));
                }
                continue;
            };
            let age = now.saturating_duration_since(asked);
            let holder_live = peers.iter().find(|other| other.key == holder);
            // A block the player is blocked on, stuck with a peer that has gone
            // quieter than this one, moves.
            let after = if req < focus + FOCUS_BLOCKS { steal_after / 2 } else { steal_after };
            let urgent = pieces.get(piece as usize).is_some_and(|p| p.priority >= PiecePriority::Now);
            let quieter = peer.last_useful.is_some_and(|at| now.saturating_duration_since(at) <= after)
                && holder_live.is_none_or(|h| peer.last_useful_ms() > h.last_useful_ms());
            if !steal_after.is_zero() && urgent && age >= after && quieter {
                pick.blocks.push((req, Take::From(holder)));
                continue;
            }
            if !tuning.hedge || self.blocks.second(req).is_some() || mine == f64::MAX {
                continue;
            }
            let rtt = holder_live.map_or(Duration::ZERO, |h| h.rtt());
            let overdue = age >= OVERDUE_AFTER.max(rtt.saturating_mul(2));
            if !overdue && !(eager && req < focus + SEEK_COPY_BLOCKS) {
                continue;
            }
            // The quickest other peer sends the copy; the rest stay out of it.
            let quickest = !peers.iter().any(|other| other.key != peer.key
                && other.key != holder
                && can_serve(other, piece)
                && other.arrival() < mine);
            if quickest {
                pick.blocks.push((req, Take::Copy { eager: !overdue }));
            }
        }
    }

    /// The queue: best piece first, each block handed to one peer.
    #[allow(clippy::too_many_arguments)]
    fn pick_queue(&self, peer: &PickPeer<'_>, want: usize, window: Option<std::ops::Range<u32>>,
        open_limit: usize, wanted: &crate::torrent::WantedPieces, plen: i64, total: i64,
        pick: &mut Pick) {
        let tuning = crate::tuning::current_seek_tuning();
        let slow = tuning.slow_peer_yield
            && peer.last_useful.is_some()
            && peer.rate < SLOW_PEER_BYTES_PER_SEC;
        let mut yields = SLOW_PEER_YIELDS;
        let mut queue = self.saved_queue.lock();
        let queue = &mut *queue;
        let rewind = queue.next.len() != wanted.num_pieces as usize
            || queue.rewound_at.is_none_or(|at| at.elapsed() >= REWIND_EVERY);
        if rewind {
            queue.next.clear();
            queue.next.resize(wanted.num_pieces as usize, 0);
            queue.rewound_at = Some(Instant::now());
        }
        if rewind || queue.generation != wanted.generation {
            queue.generation = wanted.generation;
            queue.open = wanted.checking
                + wanted.candidates.iter()
                    .filter(|c| c.2 || queue.next.get(c.0 as usize).is_some_and(|&n| n > 0))
                    .count();
        }
        for &(index, priority, partial, chunks, _) in &wanted.candidates {
            let Some(&from) = queue.next.get(index as usize) else { continue };
            // Handed out in full. Interest may still need the piece looked at.
            if from as usize >= chunks && pick.interested {
                continue;
            }
            if !peer.has(index) {
                continue;
            }
            pick.interested = true;
            if pick.blocks.len() >= want {
                break;
            }
            if peer.blocked(index) {
                if peer.allowed_fast.is_empty() {
                    break;
                }
                continue;
            }
            // RAM is spoken for: finish what is open. A peer about to run dry
            // may still start a piece, so slow holders never idle a fast peer.
            let fresh = !partial && from == 0;
            if fresh && queue.open >= open_limit && !peer.starving && priority < PiecePriority::Now {
                break;
            }
            if slow && priority == PiecePriority::Next && yields > 0
                && self.faster_peers_finish(peer, index, plen, total)
            {
                yields -= 1;
                continue;
            }
            let (begin, end) = piece_req_index_range(index, plen, total);
            let taken = pick.blocks.len();
            let mut at = begin + from;
            while at < end && pick.blocks.len() < want {
                let req = at;
                at += 1;
                if window.as_ref().is_some_and(|w| w.contains(&req))
                    || peer.cancelled.contains(&req)
                    || peer.outstanding.contains(&req)
                    || !self.block_free(req)
                {
                    continue;
                }
                pick.blocks.push((req, Take::Free));
            }
            queue.next[index as usize] = at - begin;
            if fresh && pick.blocks.len() > taken {
                queue.open += 1;
            }
        }
    }

    /// Whether faster peers can finish this piece before this slow peer's queue
    /// would get to it.
    fn faster_peers_finish(&self, peer: &PickPeer<'_>, piece: PieceIndex, plen: i64, total: i64) -> bool {
        let (begin, end) = piece_req_index_range(piece, plen, total);
        let mut missing = (begin..end)
            .filter(|&r| !self.dirty_chunks.contains(r))
            .map(|r| f64::from(block_coord_from_req_index(r, plen, total).length))
            .sum::<f64>();
        let speed = peer.rate.max(1.0);
        let secs = peer.outstanding.len().max(1) as f64 * f64::from(BLOCK_SIZE) / speed;
        let peers = self.live_peers.read();
        let bitfields = self.peer_pieces.read();
        for other in peers.iter() {
            let rate = other.rate();
            if other.key == peer.key
                || other.choking.load(Ordering::Relaxed)
                || rate < SLOW_PEER_BYTES_PER_SEC
                || rate <= speed
                || !(other.have_all.load(Ordering::Relaxed)
                    || bitfields.get(&other.key).is_some_and(|b| b.contains(&piece)))
            {
                continue;
            }
            missing -= rate * secs;
            if missing <= 0.0 {
                return true;
            }
        }
        false
    }

    /// The very end is small enough to read whole. Free blocks first; without
    /// any, second copies so one slow holder cannot hold up the end.
    fn pick_endgame(&self, peer: &PickPeer<'_>, want: usize, wanted: &crate::torrent::WantedPieces,
        plen: i64, total: i64, pick: &mut Pick) {
        let mut copies = Vec::new();
        for &(index, ..) in &wanted.candidates {
            if !peer.has(index) {
                continue;
            }
            pick.interested = true;
            if peer.blocked(index) {
                continue;
            }
            let (begin, end) = piece_req_index_range(index, plen, total);
            for req in begin..end {
                if pick.blocks.len() >= want {
                    return;
                }
                if self.dirty_chunks.contains(req)
                    || self.blocks.is_writing(req)
                    || peer.cancelled.contains(&req)
                    || peer.outstanding.contains(&req)
                {
                    continue;
                }
                if self.blocks.is_owned(req) {
                    if copies.len() < want {
                        copies.push((req, Take::Copy { eager: false }));
                    }
                } else {
                    pick.blocks.push((req, Take::Free));
                }
            }
        }
        if pick.blocks.is_empty() {
            pick.blocks = copies;
        }
    }

    /// This peer has nothing to do while slower peers sit on blocks.
    fn pick_stalled(&self, peer: &PickPeer<'_>, want: usize, wanted: &crate::torrent::WantedPieces,
        plen: i64, total: i64, pick: &mut Pick) {
        let now = Instant::now();
        let peers = self.live_peers.read();
        for &(index, ..) in wanted.candidates.iter().take(STALLED_SCAN_PIECES) {
            if !peer.has(index) {
                continue;
            }
            let (begin, end) = piece_req_index_range(index, plen, total);
            for req in begin..end {
                if !self.blocks.is_owned(req)
                    || peer.outstanding.contains(&req)
                    || peer.cancelled.contains(&req)
                {
                    continue;
                }
                let Some((holder, asked)) = self.blocks.owner(req) else { continue };
                if holder == peer.key || now.saturating_duration_since(asked) < STALLED_AFTER {
                    continue;
                }
                let holder_rate = peers.iter().find(|p| p.key == holder).map_or(0.0, |p| p.rate());
                if peer.rate > holder_rate * 2.0 {
                    pick.blocks.push((req, Take::From(holder)));
                    if pick.blocks.len() >= want {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> BlockTable {
        let table = BlockTable::default();
        table.init(1024);
        table
    }

    #[test]
    fn only_one_peer_owns_a_block() {
        let t = table();
        let now = Instant::now();
        assert!(t.claim(1, 10, now));
        assert!(t.claim(1, 10, now), "the owner may claim again");
        assert!(!t.claim(1, 20, now));
        assert!(t.is_owned(1));
        t.release(1, 20);
        assert!(t.is_owned(1), "only the owner releases");
        t.release(1, 10);
        assert!(!t.is_owned(1));
        assert!(t.claim(1, 20, now));
    }

    #[test]
    fn one_second_copy_survives_its_owner() {
        let t = table();
        let now = Instant::now();
        assert!(t.claim(5, 10, now));
        assert!(t.claim_second(5, 20));
        assert!(!t.claim_second(5, 30));
        t.release(5, 10);
        assert_eq!(t.second(5), Some(20));
        assert!(t.holds(5, 20));
        assert!(!t.holds(5, 10));
        t.release_second(5, 20);
        assert!(!t.holds(5, 20));
    }

    #[test]
    fn a_leaving_peer_hands_everything_back() {
        let t = table();
        let now = Instant::now();
        assert!(t.claim(1, 10, now));
        assert!(t.claim(2, 20, now));
        assert!(t.claim_second(2, 10));
        t.forget_peer(10);
        assert!(!t.is_owned(1));
        assert!(t.is_owned(2));
        assert_eq!(t.second(2), None);
    }

    #[test]
    fn one_writer_per_block() {
        let t = table();
        assert!(t.begin_write(3));
        assert!(!t.begin_write(3));
        assert!(t.is_writing(3));
        t.end_write(3);
        assert!(t.begin_write(3));
    }

    #[test]
    fn queue_limit_follows_rate_and_round_trip() {
        let rtt = Duration::from_millis(100);
        assert_eq!(peer_queue_limit(0.0, rtt, true), UNMEASURED_QUEUE);
        assert_eq!(peer_queue_limit(1_000.0, Duration::ZERO, true), UNMEASURED_QUEUE);
        assert_eq!(peer_queue_limit(1_048_576.0, rtt, true), 14);
        assert_eq!(peer_queue_limit(1_000.0, rtt, false), QUEUE_FLOOR);
        // A long round trip lengthens the saved-download queue past 3 s of rate.
        assert_eq!(peer_queue_limit(1_638_400.0, Duration::from_secs(3), false), 600);
    }

    #[test]
    fn unmeasured_peers_arrive_last() {
        assert_eq!(expected_arrival(0.0, Duration::ZERO, 0), f64::MAX);
        assert!(expected_arrival(1e6, Duration::from_millis(50), 0)
            < expected_arrival(1e6, Duration::from_millis(50), 100));
    }
}

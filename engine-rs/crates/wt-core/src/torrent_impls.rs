//! `PeerTorrent` for [`TorrentInner`] (and ReaderHost ownership note).
//!
//! ## Status (pause checkpoint)
//!
//! Implements [`PeerTorrent`] against **currently available** `TorrentInner`
//! fields so `wt-core` compiles. Full `PieceBook` / `StorageGlue` / hasher /
//! metadata / PEX wiring is sketched in comments and helpers; enable once
//! agy-2 adds the `pub(crate)` fields listed in `HANDOFF-grok-9.md`.
//!
//! `impl ReaderHost for TorrentInner` currently lives in `torrent.rs` (agy-2).
//! On resume, delete that impl and restore the book-backed ReaderHost from
//! git history / HANDOFF — ownership belongs here.
//!
//! ## Method coverage (PeerTorrent)
//!
//! | Group | Status |
//! |---|---|
//! | Info / lengths / flags | full |
//! | Chunk dirty via `dirty_chunks` | full |
//! | write/read via `storage` | full |
//! | Assignments via `request_state` | full (PeerKey ↔ addr) |
//! | Hedges via `hedges` | full |
//! | desired_request_context | full (ephemeral order) |
//! | peer_has_piece / webtor snap | full (via `conns`) |
//! | queue_piece_hash | TODO no pool field — sets `checking` on PieceEntry |
//! | metadata / PEX / DHT lists | TODO stubs (compile-safe) |
//! | choke_config | default + live disallow/seed flags |
//! | ReaderHost | deferred — see `torrent.rs` until fields land |

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use wt_types::{PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};
use wt_wire::ext::PexMsg;

use crate::choking::ChokeConfig;
use crate::peer_conn::{PeerKey, PeerTorrent, WebtorPeerSnap};
use crate::picker::req_index_from_piece_and_offset;
use crate::reader::{reader_piece_priorities, PieceRange, ReaderHost};
use crate::strategy::{
    PeerRequestAssignment, PieceRequestOrder, PieceRequestOrderKey, PieceRequestOrderState,
    RequestTorrentSnapshot, StrategyLimits,
};
use crate::torrent::TorrentInner;
use crate::tuning::{current_seek_tuning, SeekTuning};
use crate::{CoreError, Result};

/// Stable PeerKey derived from a socket address.
pub fn peer_key_from_addr(addr: SocketAddr) -> PeerKey {
    let port = u64::from(addr.port());
    match addr.ip() {
        std::net::IpAddr::V4(v) => (u64::from(u32::from(v)) << 16) | port,
        std::net::IpAddr::V6(v) => {
            let mut h = 0u64;
            for b in v.octets() {
                h = h.wrapping_mul(16777619) ^ u64::from(b);
            }
            (h << 16) | port
        }
    }
}

const READ_NUDGE_EVERY: Duration = Duration::from_millis(50);

fn addr_for_key(t: &TorrentInner, key: PeerKey) -> Option<SocketAddr> {
    if let Some(&addr) = t.peer_addrs.read().get(&key) {
        return Some(addr);
    }
    if let Some(&addr) = t.conn_handles.read().keys().find(|&&a| peer_key_from_addr(a) == key) {
        return Some(addr);
    }
    t.peer_snapshots.read().keys().copied().find(|&a| peer_key_from_addr(a) == key)
}

/// Storage of `piece` for a received block of `len` bytes at `begin`.
fn chunk_storage(
    t: &TorrentInner,
    piece: PieceIndex,
    begin: u32,
    len: usize,
) -> Result<Arc<dyn wt_types::PieceStorage>> {
    let info = t.info.read().clone().ok_or(CoreError::NoInfo)?;
    if piece >= info.num_pieces()
        || u64::from(begin) + len as u64 > info.piece(piece).length as u64
    {
        return Err(CoreError::Other("invalid piece range".into()));
    }
    let storage = t
        .storage
        .read()
        .clone()
        .ok_or_else(|| CoreError::Other("no storage".into()))?;
    Ok(storage.piece(info.piece(piece), info.piece_hash_v1(piece)))
}

fn check_full_write(n: usize, len: usize) -> Result<()> {
    if n != len {
        return Err(CoreError::Other(format!("short write: {n} of {len}")));
    }
    Ok(())
}

impl PeerTorrent for TorrentInner {
    fn peer_key(&self, addr: SocketAddr) -> PeerKey {
        peer_key_from_addr(addr)
    }

    fn have_info(&self) -> bool {
        self.info.read().is_some()
    }

    fn num_pieces(&self) -> u32 {
        self.pieces.read().len() as u32
    }

    fn piece_length(&self) -> i64 {
        self.info
            .read()
            .as_ref()
            .map(|i| i.piece_length)
            .unwrap_or(0)
    }

    fn total_length(&self) -> i64 {
        self.info
            .read()
            .as_ref()
            .map(|i| i.total_length())
            .unwrap_or(0)
    }

    fn chunk_size(&self) -> u32 {
        BLOCK_SIZE
    }

    fn info_hash_bytes(&self) -> [u8; 20] {
        self.info_hash.0
    }

    fn data_download_disallowed(&self) -> bool {
        self.data_download_disallowed.load(Ordering::Relaxed)
    }

    fn data_upload_disallowed(&self) -> bool {
        self.data_upload_disallowed.load(Ordering::Relaxed)
    }

    fn is_seeding(&self) -> bool {
        self.seeding.load(Ordering::Relaxed) || {
            let pieces = self.pieces.read();
            !pieces.is_empty() && pieces.iter().all(|p| p.complete)
        }
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn piece_complete(&self, piece: PieceIndex) -> bool {
        self.pieces
            .read()
            .get(piece as usize)
            .map(|p| p.complete)
            .unwrap_or(false)
    }

    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority {
        self.pieces
            .read()
            .get(piece as usize)
            .map(|p| p.priority)
            .unwrap_or(PiecePriority::None)
    }

    fn piece_hashing(&self, piece: PieceIndex) -> bool {
        self.pieces
            .read()
            .get(piece as usize)
            .is_some_and(|p| p.checking)
    }

    fn chunk_dirty(&self, req: RequestIndex) -> bool {
        self.dirty_chunks.contains(req)
    }

    fn mark_chunk_dirty(&self, req: RequestIndex) {
        self.dirty_chunks.insert(req);
        let piece = self.info.read().as_ref().map(|info| {
            crate::picker::block_coord_from_req_index(req, info.piece_length, info.total_length())
                .piece as usize
        });
        // Only a piece's first block changes what refills see. `pieces_mut`
        // on every block threw away the scan all peers share.
        if let Some(piece) = piece {
            if self.pieces.read().get(piece).is_some_and(|p| !p.partial) {
                if let Some(entry) = self.pieces_mut().get_mut(piece) {
                    entry.partial = true;
                }
            }
        }
        // Readable data changed; no new request work for other peers.
        TorrentInner::notify_progress(self);
    }

    fn begin_chunk_write(&self, req: RequestIndex) -> bool {
        // Complete/hashing pieces have every block dirty; checked unlocked
        // first so only `pending → dirty` locks nest below.
        let piece = self.info.read().as_ref().map(|info| {
            crate::picker::block_coord_from_req_index(req, info.piece_length, info.total_length())
                .piece
        });
        if piece.is_some_and(|piece| {
            self.pieces
                .read()
                .get(piece as usize)
                .is_some_and(|p| p.complete || p.checking)
        }) {
            return false;
        }
        let mut pending = self.pending_chunk_writes.lock();
        if pending.contains(&req) || self.dirty_chunks.contains(req) {
            return false;
        }
        pending.insert(req)
    }

    fn finish_chunk_write(&self, req: RequestIndex, written: bool) {
        // Dirty before releasing the reservation: a concurrent duplicate sees
        // one state or the other, never neither.
        if written {
            self.mark_chunk_dirty(req);
        }
        self.pending_chunk_writes.lock().remove(&req);
    }

    fn any_wanted_incomplete(&self) -> bool {
        self.wanted_pieces().incomplete > 0
    }

    fn any_wanted_among(&self, wanted: &std::collections::HashSet<PieceIndex>) -> bool {
        let pieces = self.pieces.read();
        wanted.iter().any(|&i| {
            pieces
                .get(i as usize)
                .is_some_and(|p| p.priority > PiecePriority::None && !p.complete)
        })
    }

    fn background_only(&self) -> bool {
        // By what is wanted, not whether a playhead is set: any brief read, such
        // as a thumbnail, sets one, and it stays while the download runs.
        self.saved_mode.load(Ordering::Relaxed)
            && self.wanted_pieces()
                .candidates
                .first()
                .is_none_or(|c| c.1 < PiecePriority::Readahead)
    }

    fn background_pick(
        &self,
        peer: &crate::peer_conn::BackgroundPeer<'_>,
        want: usize,
    ) -> crate::peer_conn::BackgroundPick {
        /// How soon a block handed back by a choke or a dropped peer is seen.
        const REWIND_EVERY: Duration = Duration::from_millis(250);
        let mut pick = crate::peer_conn::BackgroundPick::default();
        let Some(info) = self.info.read().clone() else {
            return pick;
        };
        let (piece_length, total_length) = (info.piece_length, info.total_length());
        let wanted = self.wanted_pieces();
        pick.wanted_bytes = i64::from(wanted.count).saturating_mul(piece_length);
        let writing = self.pending_chunk_writes.lock().clone();
        let assignments = self.assignments.read();
        let live = self.request_state.read();
        let holder = |req: RequestIndex| {
            assignments
                .get(&req)
                .map(|&(owner, _)| owner)
                .or_else(|| live.get(&req).map(|a| peer_key_from_addr(a.peer)))
        };

        // The very end is small enough to read whole, and idle peers need to
        // know what the others hold to take a second copy.
        if pick.wanted_bytes <= crate::peer_conn::ENDGAME_BYTES {
            'endgame: for &(index, _, _, _, _) in &wanted.candidates {
                if !peer.have_all && !peer.pieces.contains(&index) {
                    continue;
                }
                pick.interested = true;
                if peer.choking && !peer.allowed_fast.contains(&index) {
                    continue;
                }
                let (begin, end) = crate::picker::piece_req_index_range(index, piece_length, total_length);
                for req in begin..end {
                    if self.dirty_chunks.contains(req) || writing.contains(&req) || peer.cancelled.contains(&req) {
                        continue;
                    }
                    match holder(req) {
                        Some(owner) if owner == peer.key => {}
                        Some(_) => {
                            if pick.held.len() < want {
                                pick.held.push(req);
                            }
                        }
                        None => {
                            pick.free.push(req);
                            if pick.free.len() >= want.max(1) {
                                break 'endgame;
                            }
                        }
                    }
                }
            }
            return pick;
        }

        let open_limit = (crate::strategy::SAVED_MAX_UNVERIFIED_BYTES / piece_length.max(1))
            .max(crate::strategy::SAVED_OPEN_MIN_PIECES) as usize;
        let mut queue = self.saved_queue.lock();
        let queue = &mut *queue;
        let rewind = queue.next.len() != wanted.num_pieces as usize
            || queue.rewound_at.is_none_or(|at| at.elapsed() >= REWIND_EVERY);
        if rewind {
            queue.next.clear();
            queue.next.resize(wanted.num_pieces as usize, 0);
            queue.rewound_at = Some(Instant::now());
            wt_types::probe::add(wt_types::probe::REWINDS, 1);
        }
        if rewind || queue.generation != wanted.generation {
            queue.generation = wanted.generation;
            queue.open = wanted.checking
                + wanted.candidates.iter()
                    .filter(|c| c.2 || queue.next.get(c.0 as usize).is_some_and(|&n| n > 0))
                    .count();
        }
        'pieces: for &(index, _, partial, _, _) in &wanted.candidates {
            if !peer.have_all && !peer.pieces.contains(&index) {
                continue;
            }
            pick.interested = true;
            if want == 0 {
                break;
            }
            if peer.choking && !peer.allowed_fast.contains(&index) {
                if peer.allowed_fast.is_empty() {
                    break;
                }
                continue;
            }
            let Some(&from) = queue.next.get(index as usize) else {
                continue;
            };
            // RAM is spoken for: finish what is open. A peer about to run dry
            // may still start a piece, so slow holders never idle a fast peer.
            let fresh = !partial && from == 0;
            if fresh && queue.open >= open_limit {
                if !peer.starving {
                    wt_types::probe::add(wt_types::probe::OPEN_LIMIT, 1);
                    break;
                }
                wt_types::probe::add(wt_types::probe::STARVE_OPEN, 1);
            }
            let (begin, end) = crate::picker::piece_req_index_range(index, piece_length, total_length);
            let taken = pick.free.len();
            let mut at = begin + from;
            while at < end && pick.free.len() < want {
                let req = at;
                at += 1;
                if !peer.cancelled.contains(&req)
                    && !self.dirty_chunks.contains(req)
                    && !writing.contains(&req)
                    && holder(req).is_none()
                {
                    pick.free.push(req);
                }
            }
            queue.next[index as usize] = at - begin;
            if fresh && pick.free.len() > taken {
                queue.open += 1;
            }
            if pick.free.len() >= want {
                break 'pieces;
            }
        }
        pick
    }

    fn read_chunk_in_memory(&self, piece: PieceIndex, begin: u32, length: u32) -> Option<Result<Bytes>> {
        let info = self.info.read().clone()?;
        if piece >= info.num_pieces()
            || u64::from(begin) + u64::from(length) > info.piece(piece).length as u64
        {
            return None;
        }
        let storage = self.storage.read().clone()?;
        let mut buf = vec![0u8; length as usize];
        let n = storage
            .piece(info.piece(piece), info.piece_hash_v1(piece))
            .read_at_in_memory(&mut buf, i64::from(begin))?;
        Some(match n {
            Ok(n) => {
                buf.truncate(n);
                Ok(Bytes::from(buf))
            }
            Err(err) => Err(err.into()),
        })
    }

    fn clear_chunk_requested(&self, _req: RequestIndex) {}

    fn mark_chunk_requested(&self, _req: RequestIndex) {}

    fn piece_all_dirty(&self, piece: PieceIndex) -> bool {
        let info = self.info.read();
        let Some(info) = info.as_ref() else {
            return false;
        };
        let (begin, end) =
            crate::picker::piece_req_index_range(piece, info.piece_length, info.total_length());
        self.dirty_chunks.all_in_range(begin, end)
    }

    fn note_availability(&self, piece: PieceIndex, delta: i32) {
        if let Some(count) = self.piece_availability.get().and_then(|c| c.get(piece as usize)) {
            count.fetch_add(delta, Ordering::Relaxed);
        }
    }

    fn focus_block_claimed(&self) {
        if !self.seek_hedge_open() {
            return;
        }
        let now = self.warm_epoch.elapsed().as_millis() as u64;
        let last = self.last_seek_nudge_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= crate::torrent::SEEK_NUDGE_GAP_MS
            && self
                .last_seek_nudge_ms
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            self.nudge_urgent_requests();
        }
    }

    fn may_warm(&self) -> bool {
        if TorrentInner::request_focus_index(self).is_none() {
            return false;
        }
        let now = self.warm_epoch.elapsed().as_millis() as u64;
        let last = self.last_warm_ms.load(Ordering::Relaxed);
        now.saturating_sub(last) >= crate::peer_conn::WARM_TORRENT_GAP_MS
            && self
                .last_warm_ms
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
    }

    fn note_have_all_availability(&self, delta: i32) {
        self.have_all_peers.fetch_add(delta, Ordering::Relaxed);
    }

    fn note_peer_pieces(&self, peer: PeerKey, pieces: &mut dyn Iterator<Item = PieceIndex>) {
        self.peer_pieces.write().entry(peer).or_default().extend(pieces);
    }

    fn displaced_requests(
        &self,
        requests: &std::collections::HashSet<RequestIndex>,
        peer: PeerKey,
    ) -> Vec<RequestIndex> {
        // Same lock order as claim_request and claim_hedge, one pass per table
        // rather than a round of locks per request.
        let mut displaced: Vec<_> = {
            let assignments = self.assignments.read();
            let live = self.request_state.read();
            requests
                .iter()
                .copied()
                .filter(|req| match assignments.get(req) {
                    Some(&(owner, _)) => owner != peer,
                    None => live.get(req).map(|a| peer_key_from_addr(a.peer)) != Some(peer),
                })
                .collect()
        };
        if !displaced.is_empty() {
            let hedge_peers = self.hedge_peers.read();
            let hedges = self.hedges.read();
            displaced.retain(|req| {
                hedge_peers
                    .get(req)
                    .copied()
                    .or_else(|| hedges.get(req).map(|a| peer_key_from_addr(*a)))
                    != Some(peer)
            });
        }
        displaced
    }

    fn note_peer_piece(&self, peer: PeerKey, piece: PieceIndex, has: bool) {
        let mut peers = self.peer_pieces.write();
        let pieces = peers.entry(peer).or_default();
        if has {
            pieces.insert(piece);
        } else {
            pieces.remove(&piece);
        }
    }

    fn note_peer_helpful(&self, addr: SocketAddr) {
        self.clear_dial_penalty(addr);
    }

    fn peer_closed(&self, addr: SocketAddr) {
        let had_last_helpful = self
            .peer_snapshots
            .read()
            .get(&addr)
            .is_some_and(|s| s.last_useful_data.is_some());
        let old = self.retire_connection(addr);
        if let Some(handle) = old {
            let reason = handle
                .close_reason()
                .unwrap_or_else(|| "unspecified".into());
            let useful_bytes = handle
                .stats()
                .bytes_read_useful_data
                .load(Ordering::Relaxed);
            *self
                .swarm_diagnostics
                .lock()
                .closes
                .entry(crate::stats::failure_kind(&reason).into())
                .or_default() += 1;
            wt_types::events::record("peer_closed", format!(
                "kind={} useful={useful_bytes} out={}",
                crate::stats::failure_kind(&reason), handle.is_outgoing()));
            #[cfg(not(target_os = "android"))]
            eprintln!(
                "peer_closed addr={} out={} src={:?} reason={}",
                addr,
                handle.is_outgoing(),
                handle.source(),
                reason.chars().take(120).collect::<String>()
            );
            // Unhelpful reset/decode/eof escalate fail_count; dial_ok no longer
            // clears it — only note_peer_helpful does.
            self.maybe_penalize_unhelpful_close(addr, &reason, useful_bytes, had_last_helpful);
            // A peer that delivered is worth redialing without waiting for a
            // tracker, DHT or PEX to report it again. Inbound addresses are
            // ephemeral, and these closes were deliberate.
            let deliberate = ["dropped by caller", "torrent dropped", "client closed", "network changed"]
                .iter()
                .any(|r| reason.contains(r));
            if handle.is_outgoing()
                && useful_bytes > 0
                && !deliberate
                && !self.closed.load(Ordering::Relaxed)
            {
                self.dial_backoff.lock().insert(addr, Instant::now() + Duration::from_secs(5));
                self.add_peers(vec![wt_types::PeerInfo {
                    id: handle.peer_id(),
                    addr,
                    source: handle.source(),
                    supports_encryption: handle.is_encrypted(),
                    pex: Default::default(),
                    trusted: false,
                }]);
            }
        }
        self.peer_pieces.write().remove(&peer_key_from_addr(addr));
        self.peer_snapshots.write().remove(&addr);
        self.forget_peer_playback_state(addr);
        self.choked_since.write().remove(&addr);
        self.bump_change();
    }

    fn update_peer_snapshot(&self, info: crate::stats::PeerConnInfo) {
        if info.peer_choking != self.choked_since.read().contains_key(&info.remote_addr) {
            let mut since = self.choked_since.write();
            if info.peer_choking {
                since.entry(info.remote_addr).or_insert_with(Instant::now);
            } else {
                since.remove(&info.remote_addr);
            }
        }
        self.peer_snapshots.write().insert(info.remote_addr, info);
    }

    fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()> {
        let n = chunk_storage(self, piece, begin, data.len())?.write_at(data, i64::from(begin))?;
        check_full_write(n, data.len())
    }

    fn write_chunk_in_memory(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Option<Result<()>> {
        let storage = match chunk_storage(self, piece, begin, data.len()) {
            Ok(storage) => storage,
            Err(err) => return Some(Err(err)),
        };
        let written = storage.write_at_in_memory(data, i64::from(begin))?;
        Some(
            written
                .map_err(CoreError::from)
                .and_then(|n| check_full_write(n, data.len())),
        )
    }

    fn report_write_chunk_error(&self, err: &CoreError) {
        if let Some(cb) = &*self.on_write_chunk_error.read() {
            cb(err);
        }
    }

    fn read_chunk(&self, piece: PieceIndex, begin: u32, length: u32) -> Result<Bytes> {
        let info = self.info.read().clone().ok_or(CoreError::NoInfo)?;
        if piece >= info.num_pieces()
            || u64::from(begin) + u64::from(length) > info.piece(piece).length as u64
        {
            return Err(CoreError::Other("invalid piece range".into()));
        }
        let storage = self
            .storage
            .read()
            .clone()
            .ok_or_else(|| CoreError::Other("no storage".into()))?;
        let mut buf = vec![0u8; length as usize];
        let n = storage
            .piece(info.piece(piece), info.piece_hash_v1(piece))
            .read_at(&mut buf, i64::from(begin))?;
        buf.truncate(n);
        Ok(Bytes::from(buf))
    }

    fn queue_piece_hash(&self, piece: PieceIndex) {
        let Some(info) = self.info.read().clone() else {
            return;
        };
        let Some(storage) = self.storage.read().clone() else {
            return;
        };
        let Some(hash) = info.piece_hash_v1(piece) else {
            return;
        };
        let priority = {
            let mut pieces = self.pieces_mut();
            let Some(p) = pieces.get_mut(piece as usize) else {
                return;
            };
            if p.checking || p.complete {
                return;
            }
            p.checking = true;
            p.priority
        };
        let piece_obj = info.piece(piece);
        let job = crate::hashing::HashJob {
            piece_index: piece,
            priority,
            storage: storage.piece(piece_obj, Some(hash)),
            expected: crate::hashing::ExpectedHash::V1(hash),
            piece_length: info.piece_length,
            data_length: piece_obj.length,
            v1_zero_pad: 0,
            smartban: None,
            seq: self.hash_seq.fetch_add(1, Ordering::Relaxed),
        };
        if !self
            .hasher
            .lock()
            .as_ref()
            .is_some_and(|pool| pool.queue(job).is_ok())
        {
            if let Some(p) = self.pieces_mut().get_mut(piece as usize) {
                p.checking = false;
            }
            // Queueing failed: the piece is requestable again.
            TorrentInner::bump_change(self);
            return;
        }
        TorrentInner::notify_progress(self);
    }

    fn requesting_peer(&self, req: RequestIndex) -> Option<PeerKey> {
        if let Some((peer, _)) = self.assignments.read().get(&req) {
            return Some(*peer);
        }
        self.request_state
            .read()
            .get(&req)
            .map(|a| peer_key_from_addr(a.peer))
    }

    fn request_when(&self, req: RequestIndex) -> Option<Instant> {
        if let Some((_, when)) = self.assignments.read().get(&req) {
            return Some(*when);
        }
        self.request_state.read().get(&req).map(|a| a.when)
    }

    fn assign_request(&self, req: RequestIndex, peer: PeerKey, when: Instant) {
        self.assignments.write().insert(req, (peer, when));
        if let Some(addr) = addr_for_key(self, peer) {
            self.request_state.write().insert(
                req,
                crate::picker::ActiveRequest {
                    peer: addr,
                    when,
                    cancelled: false,
                },
            );
        }
    }

    fn clear_assignment(&self, req: RequestIndex) {
        let mut assignments = self.assignments.write();
        assignments.remove(&req);
        self.request_state.write().remove(&req);
    }

    fn claim_request(&self, req: RequestIndex, peer: PeerKey, when: Instant) -> bool {
        let addr = addr_for_key(self, peer);
        let mut assignments = self.assignments.write();
        let writing = self.pending_chunk_writes.lock().contains(&req);
        if writing
            || assignments
                .get(&req)
                .is_some_and(|&(owner, _)| owner != peer)
            || self.dirty_chunks.contains(req)
        {
            return false;
        }
        assignments.insert(req, (peer, when));
        if let Some(addr) = addr {
            self.request_state.write().insert(
                req,
                crate::picker::ActiveRequest {
                    peer: addr,
                    when,
                    cancelled: false,
                },
            );
        }
        true
    }

    fn release_request(&self, req: RequestIndex, peer: PeerKey) {
        let mut assignments = self.assignments.write();
        if assignments
            .get(&req)
            .is_some_and(|&(owner, _)| owner == peer)
        {
            assignments.remove(&req);
            self.request_state.write().remove(&req);
        }
    }

    fn desired_request_context(&self) -> (RequestTorrentSnapshot, PieceRequestOrder) {
        self.build_request_context(None)
    }

    fn desired_request_context_for_peer(
        &self,
        peer: &crate::strategy::DesiredPeerSnapshot,
    ) -> (RequestTorrentSnapshot, PieceRequestOrder) {
        self.build_request_context(Some(peer))
    }

    fn request_focus_index(&self) -> Option<RequestIndex> {
        TorrentInner::request_focus_index(self)
    }

    fn seek_hedge_open(&self) -> bool {
        self.seek_hedge
            .lock()
            .is_some_and(|(at, left)| left > 0 && at.elapsed() < crate::torrent::SEEK_HEDGE_WINDOW)
    }

    fn take_seek_hedge(&self) -> bool {
        match &mut *self.seek_hedge.lock() {
            Some((at, left)) if *left > 0 && at.elapsed() < crate::torrent::SEEK_HEDGE_WINDOW => {
                *left -= 1;
                true
            }
            _ => false,
        }
    }

    fn request_times(&self, peer: PeerKey) -> Vec<Instant> {
        let mut times: Vec<Instant> = self.assignments
            .read()
            .values()
            .filter(|&&(owner, _)| owner == peer)
            .map(|&(_, at)| at)
            .collect();
        times.sort_unstable();
        times
    }

    fn refund_seek_hedge(&self) {
        if let Some((_, left)) = &mut *self.seek_hedge.lock() {
            *left += 1;
        }
    }

    fn urgent_steal_after(&self) -> Duration {
        // Go `Client.urgentStealAge` from SetUrgentRequestStealing (claudeSeek).
        // Do not map HOLAfterMs here — that times HOL rescue/hedge only.
        self.client
            .read()
            .upgrade()
            .map(|c| *c.urgent_steal_after.read())
            .unwrap_or(Duration::ZERO)
    }

    fn seek_tuning(&self) -> SeekTuning {
        current_seek_tuning()
    }

    fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey> {
        if let Some(peer) = self.hedge_peers.read().get(&req) {
            return Some(*peer);
        }
        self.hedges.read().get(&req).map(|a| peer_key_from_addr(*a))
    }

    fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey) {
        let addr = addr_for_key(self, peer);
        let mut peers = self.hedge_peers.write();
        peers.insert(req, peer);
        if let Some(addr) = addr {
            self.hedges.write().insert(req, addr);
        }
    }

    fn claim_hedge(&self, req: RequestIndex, peer: PeerKey) -> bool {
        let addr = addr_for_key(self, peer);
        let mut hedges = self.hedge_peers.write();
        if hedges.contains_key(&req) || self.dirty_chunks.contains(req) {
            return false;
        }
        hedges.insert(req, peer);
        if let Some(addr) = addr {
            self.hedges.write().insert(req, addr);
        }
        true
    }

    fn clear_hedge(&self, req: RequestIndex) {
        let mut peers = self.hedge_peers.write();
        peers.remove(&req);
        self.hedges.write().remove(&req);
    }

    fn release_hedge(&self, req: RequestIndex, peer: PeerKey) {
        let mut peers = self.hedge_peers.write();
        if peers.get(&req) == Some(&peer) {
            peers.remove(&req);
            self.hedges.write().remove(&req);
        }
    }

    fn record_rescue(&self) {
        self.rescues.fetch_add(1, Ordering::Relaxed);
    }

    fn record_hedge(&self) {
        self.hedges_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn peer_has_piece(&self, peer: PeerKey, piece: PieceIndex) -> bool {
        if self
            .peer_pieces
            .read()
            .get(&peer)
            .is_some_and(|pieces| pieces.contains(&piece))
        {
            return true;
        }
        if let Some(snap) = self
            .peer_snapshots
            .read()
            .values()
            .find(|p| peer_key_from_addr(p.remote_addr) == peer)
        {
            if snap.peer_completion >= 1.0 {
                return true;
            }
        }
        false
    }

    fn webtor_peer_snap(&self, peer: PeerKey) -> Option<WebtorPeerSnap> {
        let addr = addr_for_key(self, peer)?;
        let snapshots = self.peer_snapshots.read();
        let info = snapshots.get(&addr)?;
        Some(WebtorPeerSnap {
            key: peer,
            download_rate_bps: info.download_rate,
            rtt: info.rtt,
            queued: info.queued as u32,
            cancelled: info.cancelled as u32,
            nominal_max: info.nominal_max_requests as u32,
            peer_choking: info.peer_choking,
            closed: false,
            last_useful: info
                .last_useful_data
                .and_then(|d| Instant::now().checked_sub(d)),
        })
    }

    fn connected_peer_keys(&self) -> Vec<PeerKey> {
        self.conn_handles
            .read()
            .keys()
            .map(|&addr| peer_key_from_addr(addr))
            .collect()
    }

    fn arrival_net(&self) -> Option<crate::arrival::ArrivalNet> {
        Some(self.arrival_model.read().net.clone())
    }

    fn learn_arrival(&self, sample: crate::arrival::ArrivalSample, actual: f64) {
        self.arrival_model
            .write()
            .on_block_learned(sample.x, sample.estimate, actual);
    }

    fn register_peer_arrival(
        &self,
        peer: PeerKey,
        state: Arc<parking_lot::RwLock<crate::arrival::PeerArrivalState>>,
    ) {
        self.peer_arrivals.write().insert(peer, state);
    }

    fn peer_arrival(
        &self,
        peer: PeerKey,
    ) -> Option<Arc<parking_lot::RwLock<crate::arrival::PeerArrivalState>>> {
        self.peer_arrivals.read().get(&peer).cloned()
    }

    fn on_metadata_data(&self, piece: u32, total_size: u32, data: Bytes) -> Result<bool> {
        let verified = {
            let mut metadata = self.metadata.write();
            metadata
                .set_size(total_size)
                .map_err(|e| CoreError::Other(e.to_string()))?;
            metadata
                .on_data(piece, &data)
                .map_err(|e| CoreError::Other(e.to_string()))?
        };
        if let Some(verified) = verified {
            self.set_info(Arc::new(verified.info))?;
            *self.metainfo_raw.write() = Some(wt_types::MetaInfo {
                info_bytes: verified.info_bytes,
                ..Default::default()
            });
            return Ok(true);
        }
        Ok(false)
    }

    fn metadata_piece(&self, piece: u32) -> Option<(u32, Bytes)> {
        let raw = self.metainfo_raw.read();
        let bytes = &raw.as_ref()?.info_bytes;
        let start = (piece as usize).checked_mul(crate::metadata::METADATA_PIECE_SIZE)?;
        if start >= bytes.len() {
            return None;
        }
        let end = (start + crate::metadata::METADATA_PIECE_SIZE).min(bytes.len());
        Some((
            bytes.len() as u32,
            Bytes::copy_from_slice(&bytes[start..end]),
        ))
    }

    fn metadata_size(&self) -> Option<u32> {
        self.metainfo_raw
            .read()
            .as_ref()
            .map(|m| m.info_bytes.len() as u32)
            .or_else(|| self.metadata.read().size())
    }

    fn has_metadata(&self) -> bool {
        self.info.read().is_some()
    }

    fn ensure_metadata_size(&self, size: u32) -> Result<()> {
        self.metadata
            .write()
            .set_size(size)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    fn metadata_pieces_to_request(&self, peer: &crate::metadata::PeerMetadataState) -> Vec<u32> {
        self.metadata.read().pieces_to_request(peer)
    }

    fn extended_handshake_client_version(&self) -> String {
        self.client
            .read()
            .upgrade()
            .map(|c| c.config.extended_handshake_client_version.clone())
            .unwrap_or_else(|| "Webtor 2.0".to_string())
    }

    fn listen_port(&self) -> u16 {
        self.client
            .read()
            .upgrade()
            .map(|c| *c.listen_port.read())
            .unwrap_or(0)
    }

    fn pex_disabled(&self) -> bool {
        // BEP 27: private torrents never exchange peers.
        self.info
            .read()
            .as_ref()
            .is_some_and(|i| i.private == Some(true))
            || self
                .client
                .read()
                .upgrade()
                .map(|c| c.config.disable_pex)
                .unwrap_or(false)
    }

    fn prefers_encryption(&self) -> bool {
        self.client
            .read()
            .upgrade()
            .map(|c| c.config.header_obfuscation_policy.preferred)
            .unwrap_or(true)
    }

    fn dht_port(&self) -> Option<u16> {
        let client = self.client.read().upgrade()?;
        if client.config.no_dht {
            return None;
        }
        let port = *client.listen_port.read();
        if port == 0 {
            None
        } else {
            Some(port)
        }
    }

    fn ingest_pex(&self, from: SocketAddr, msg: &PexMsg) -> Result<()> {
        let mut last = self.pex_last_recv.lock();
        let mut prior = last.get(&from).copied();
        let peers = match self.pex.lock().ingest(msg, Instant::now(), &mut prior) {
            Ok(peers) => peers,
            // Soft discard: torrent-wide rest and per-conn spacing must not
            // close the peerconn (was mass-dropping Ubuntu peers).
            Err(
                crate::pex::PexIngestError::Cooldown
                | crate::pex::PexIngestError::TooFrequent { .. },
            ) => return Ok(()),
        };
        if let Some(time) = prior {
            last.insert(from, time);
        }
        drop(last);
        self.swarm_diagnostics.lock().pex_received += peers.len() as u64;
        // Route through add_peers for high-water trim + dialer wake.
        let _ = self.add_peers(peers);
        Ok(())
    }

    fn pex_note_connected(&self, addr: SocketAddr, encrypted: bool, utp: bool) {
        let flags = wt_types::PexFlags {
            prefers_encryption: encrypted,
            seed_only: self.seeding.load(Ordering::Relaxed),
            supports_utp: utp,
            supports_holepunch: false,
            reachable: true,
        };
        self.pex.lock().add(addr, flags);
    }

    fn pex_note_closed(&self, addr: SocketAddr, encrypted: bool, utp: bool) {
        let flags = wt_types::PexFlags {
            prefers_encryption: encrypted,
            seed_only: false,
            supports_utp: utp,
            supports_holepunch: false,
            reachable: false,
        };
        self.pex.lock().drop_listed(addr, flags);
    }

    fn pex_genmsg(&self, cursor: &mut Option<crate::pex::PexCursor>) -> Option<PexMsg> {
        let pex = self.pex.lock();
        let (msg, new_cursor) = pex.genmsg(*cursor);
        *cursor = Some(new_cursor);
        Some(msg)
    }

    fn add_dht_node(&self, addr: SocketAddr) {
        let mut nodes = self.dht_nodes.write();
        if !nodes.contains(&addr) {
            nodes.push(addr);
        }
    }

    fn on_peer_listen_port(&self, peer_addr: SocketAddr, port: u16) {
        self.peer_listen_ports.write().insert(peer_addr, port);
    }

    fn choke_config(&self) -> ChokeConfig {
        ChokeConfig {
            data_upload_disallowed: self.data_upload_disallowed.load(Ordering::Relaxed),
            seeding: self.is_seeding(),
            ..ChokeConfig::default()
        }
    }

    fn bump_change(&self) {
        TorrentInner::bump_change(self);
    }

    fn notify_progress(&self) {
        TorrentInner::notify_progress(self);
    }

    fn wake_peer(&self, peer: PeerKey) {
        let Some(addr) = addr_for_key(self, peer) else {
            return;
        };
        if let Some(handle) = self.conn_handles.read().get(&addr) {
            handle.wake();
        }
    }

    fn on_chunk_received(&self, _peer: PeerKey, req: RequestIndex, _len: usize) {
        self.clear_assignment(req);
        self.clear_hedge(req);
    }
}

/// Size the availability counters, counting bitfields that arrived before info.
pub(crate) fn init_availability(t: &TorrentInner, num_pieces: u32) {
    let counts = t.piece_availability.get_or_init(|| {
        (0..num_pieces).map(|_| std::sync::atomic::AtomicI32::new(0)).collect()
    });
    for pieces in t.peer_pieces.read().values() {
        for &piece in pieces {
            if let Some(count) = counts.get(piece as usize) {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Placeholder for Have/Bitfield → availability once `peer_pieces`/`book` exist.
pub fn note_peer_piece(t: &TorrentInner, peer: PeerKey, piece: PieceIndex, has: bool) {
    let mut peers = t.peer_pieces.write();
    let pieces = peers.entry(peer).or_default();
    let changed = if has {
        pieces.insert(piece)
    } else {
        pieces.remove(&piece)
    };
    drop(peers);
    if changed {
        t.note_availability(piece, if has { 1 } else { -1 });
    }
}

impl TorrentInner {
    fn build_request_context(
        &self,
        peer: Option<&crate::strategy::DesiredPeerSnapshot>,
    ) -> (RequestTorrentSnapshot, PieceRequestOrder) {
        let info = self.info.read().clone();
        let piece_length = info.as_ref().map_or(0, |i| i.piece_length);
        let total_length = info.as_ref().map_or(0, |i| i.total_length());
        let wanted = self.wanted_pieces();
        let num_pieces = wanted.num_pieces;
        let wanted_pieces = wanted.count;
        let mut requestable = wanted.requestable.clone();
        let unverified = wanted.unverified.clone();
        let budget = peer.map_or(0, |peer| {
            crate::strategy::candidate_budget(
                peer.nominal_max_requests,
                current_seek_tuning().bound_candidates,
            )
        });
        let mut usable = 0usize;
        let mut selected = Vec::new();
        let mut held = Vec::new();
        // Blocks mid-write are neither readable nor requestable (Go unpends
        // them before WriteAt). The set is bounded by concurrent writers.
        let writing = self.pending_chunk_writes.lock().clone();
        // Match claim/release lock order. Read the live tables without cloning
        // the swarm's full outstanding queues into every peer's snapshot.
        let live_assignments = self.assignments.read();
        let live_requests = self.request_state.read();
        // Preserve each holder's full queue length for steal ranking, but
        // allocate assignment records only for this peer's candidate blocks.
        let counts = {
            let mut cached = self.request_counts.lock();
            match cached.as_ref().filter(|(at, _)| at.elapsed() < crate::torrent::REQUEST_COUNTS_MAX_AGE) {
                Some((_, counts)) => counts.clone(),
                None => {
                    let mut counts = HashMap::<u64, u64>::new();
                    for request in live_requests.values() {
                        *counts.entry(peer_key_from_addr(request.peer)).or_default() += 1;
                    }
                    let counts = Arc::new(counts);
                    *cached = Some((Instant::now(), counts.clone()));
                    counts
                }
            }
        };
        let dirty = &self.dirty_chunks;
        let mut interest_hint = false;
        for &(index, priority, partial, chunks, availability) in &wanted.candidates {
            if budget > 0 && usable >= budget && priority < PiecePriority::Now {
                break;
            }
            if peer.is_some_and(|peer| !peer.has_piece.get(index as usize).copied().unwrap_or(false)) {
                continue;
            }
            let choked_nonfast = peer.is_some_and(|peer| {
                peer.peer_choking
                    && !peer
                        .peer_allowed_fast
                        .get(index as usize)
                        .copied()
                        .unwrap_or(false)
            });
            // One missing non-Fast block is enough to express interest while
            // choked. Scan allowed-Fast pieces without expanding the rest of
            // the torrent into blocks on every idle peer's wake.
            if choked_nonfast && interest_hint {
                continue;
            }
            let (begin, end) =
                crate::picker::piece_req_index_range(index, piece_length, total_length);
            let mut requests = Vec::with_capacity(if choked_nonfast { 1 } else { chunks });
            for req in begin..end {
                if dirty.contains(req) || writing.contains(&req) {
                    continue;
                }
                let assignment = live_requests
                    .get(&req)
                    .map(|request| {
                        let owner = peer_key_from_addr(request.peer);
                        PeerRequestAssignment {
                            peer: owner,
                            when: request.when,
                            peer_outstanding: counts.get(&owner).copied().unwrap_or(1),
                        }
                    })
                    .or_else(|| {
                        live_assignments
                            .get(&req)
                            .map(|&(owner, when)| PeerRequestAssignment {
                                peer: owner,
                                when,
                                peer_outstanding: 0,
                            })
                    });
                if let Some(assignment) = assignment {
                    held.push((req, assignment));
                }
                requests.push(req);
                if let Some(peer) = peer {
                    if !peer.has_cancelled(req)
                        && (!peer.peer_choking
                            || peer
                                .peer_allowed_fast
                                .get(index as usize)
                                .copied()
                                .unwrap_or(false)
                            || peer.has_outstanding(req))
                        && assignment.is_none_or(|a| a.peer == peer.peer_id)
                    {
                        usable += 1;
                    }
                }
                if choked_nonfast {
                    interest_hint = true;
                    break;
                }
            }
            if requests.is_empty() {
                requestable[index as usize] = false;
                continue;
            }
            selected.push((index, priority, partial, availability, requests));
        }
        drop(live_assignments);
        drop(live_requests);

        // Hash-table growth and piece-order construction belong to this
        // peer's snapshot. Do that work after releasing shared request locks
        // so other peers can claim blocks and finish received writes.
        let assignments = held.into_iter().collect();
        let mut undirtied = HashMap::with_capacity(selected.len());
        let mut piece_of_request = HashMap::with_capacity(
            selected
                .iter()
                .map(|(_, _, _, _, requests)| requests.len())
                .sum(),
        );
        let mut order = PieceRequestOrder::with_capacity(selected.len());
        for (index, priority, partial, availability, requests) in selected {
            for &request in &requests {
                piece_of_request.insert(request, index);
            }
            undirtied.insert(index, requests);
            order.add(
                PieceRequestOrderKey {
                    info_hash: self.info_hash.0,
                    index,
                },
                PieceRequestOrderState {
                    availability,
                    priority,
                    partial,
                },
            );
        }
        (
            RequestTorrentSnapshot {
                info_hash: self.info_hash.0,
                piece_length,
                num_pieces,
                piece_requestable: requestable,
                piece_count_unverified: unverified,
                undirtied_by_piece: undirtied,
                piece_of_request,
                assignments,
                focus_request: TorrentInner::request_focus_index(self),
                has_active_webseed_requests: false,
                limits: if TorrentInner::request_focus_index(self).is_some() {
                    StrategyLimits::go_default()
                } else {
                    StrategyLimits::saved_download()
                },
                wanted_pieces,
            },
            order,
        )
    }

    pub fn as_reader_host(self: &Arc<Self>) -> Arc<dyn crate::reader::ReaderHost> {
        Arc::clone(self) as Arc<dyn crate::reader::ReaderHost>
    }
}

impl ReaderHost for TorrentInner {
    fn have(&self, offset: i64, len: i64, responsive: bool) -> i64 {
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return 0;
        };
        let piece_len = info.piece_length;
        let total = info.total_length();
        let mut off = offset;
        let mut max = len;
        let mut ret = 0i64;
        let pieces = self.pieces.read();
        let dirty = &self.dirty_chunks;
        while max > 0 {
            if off < 0 || off >= total {
                break;
            }
            let piece = (off / piece_len) as u32;
            let in_piece = off % piece_len;
            let chunk_start = (in_piece / BLOCK_SIZE as i64) * BLOCK_SIZE as i64;
            let piece_end = ((piece as i64 + 1) * piece_len).min(total);
            let piece_base = piece as i64 * piece_len;
            let chunk_len = (piece_end - (piece_base + chunk_start)).min(BLOCK_SIZE as i64);
            if chunk_len <= 0 {
                break;
            }
            let complete = pieces
                .get(piece as usize)
                .map(|p| p.complete)
                .unwrap_or(false);
            if !responsive && !complete {
                break;
            }
            if !complete {
                let req = req_index_from_piece_and_offset(piece, chunk_start as u32, piece_len);
                if !dirty.contains(req) {
                    break;
                }
            }
            let len1 = chunk_len - (in_piece - chunk_start);
            if len1 <= 0 {
                break;
            }
            max -= len1;
            ret += len1;
            off += len1;
        }
        if max < 0 {
            ret += max;
        }
        ret
    }

    fn wait_change(&self, after: u64, timeout: Duration) -> u64 {
        self.wait_version(after, timeout)
    }

    fn change_version(&self) -> u64 {
        self.change_version.load(Ordering::SeqCst)
    }

    fn set_reader_positions(&self, reader_id: u64, range: PieceRange) {
        self.reader_ranges.write().insert(reader_id, range);
        let ranges: Vec<PieceRange> = self.reader_ranges.read().values().copied().collect();
        let (now, readahead) = reader_piece_priorities(&ranges);
        let mut pieces = self.pieces_mut();
        for piece in pieces.iter_mut() {
            let reader_priority = if now.contains(&piece.index) {
                PiecePriority::Now
            } else if readahead.contains(&piece.index) {
                PiecePriority::High
            } else {
                PiecePriority::None
            };
            piece.priority = piece.base_priority.max(reader_priority);
        }
        drop(pieces);
        self.bump_change();
        self.nudge_urgent_requests();
    }

    fn clear_reader(&self, reader_id: u64) {
        self.reader_ranges.write().remove(&reader_id);
        let ranges: Vec<PieceRange> = self.reader_ranges.read().values().copied().collect();
        let (now, readahead) = reader_piece_priorities(&ranges);
        let mut pieces = self.pieces_mut();
        for piece in pieces.iter_mut() {
            let reader_priority = if now.contains(&piece.index) {
                PiecePriority::Now
            } else if readahead.contains(&piece.index) {
                PiecePriority::High
            } else {
                PiecePriority::None
            };
            piece.priority = piece.base_priority.max(reader_priority);
        }
        drop(pieces);
        self.bump_change();
    }

    fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> std::io::Result<usize> {
        let info_guard = self.info.read();
        let Some(info) = &*info_guard else {
            return Ok(0);
        };
        let storage_guard = self.storage.read();
        let Some(storage) = &*storage_guard else {
            return Ok(0);
        };
        if torrent_offset < 0 || torrent_offset >= info.total_length() || buf.is_empty() {
            return Ok(0);
        }

        let piece_len = info.piece_length;
        let mut total_read = 0;
        let mut cur_offset = torrent_offset;

        while total_read < buf.len() && cur_offset < info.total_length() {
            let piece_idx = (cur_offset / piece_len) as u32;
            let offset_in_piece = cur_offset % piece_len;
            let piece_obj = info.piece(piece_idx);
            let piece_storage = storage.piece(piece_obj, info.piece_hash_v1(piece_idx));
            let bytes_in_piece = (piece_obj.length - offset_in_piece) as usize;
            let want = (buf.len() - total_read).min(bytes_in_piece);

            match piece_storage.read_at(&mut buf[total_read..total_read + want], offset_in_piece) {
                Ok(n) => {
                    if n == 0 {
                        break;
                    }
                    total_read += n;
                    cur_offset += n as i64;
                }
                Err(e) => {
                    return Err(std::io::Error::other(format!("storage read error: {e}")));
                }
            }
        }

        Ok(total_read)
    }

    fn piece_length(&self) -> Option<i64> {
        self.info.read().as_ref().map(|i| i.piece_length)
    }

    fn torrent_length(&self) -> i64 {
        self.info
            .read()
            .as_ref()
            .map(|i| i.total_length())
            .unwrap_or(0)
    }

    fn num_pieces(&self) -> u32 {
        self.info
            .read()
            .as_ref()
            .map(|i| i.num_pieces())
            .unwrap_or(0)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn has_info(&self) -> bool {
        self.info.read().is_some()
    }

    fn data_download_allowed(&self) -> bool {
        !self.data_download_disallowed.load(Ordering::Relaxed)
    }

    fn on_read_waiting(&self, torrent_offset: i64) {
        self.request_focus
            .store(torrent_offset + 1, Ordering::Relaxed);
        // This runs for every block that arrives while a read waits; each
        // nudge has every peer with spare slots rescan the torrent.
        let mut last = self.last_read_nudge.lock();
        if last.is_none_or(|at| at.elapsed() >= READ_NUDGE_EVERY) {
            *last = Some(Instant::now());
            drop(last);
            self.nudge_urgent_requests();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn peer_key_stable() {
        let a = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 6881));
        assert_eq!(peer_key_from_addr(a), peer_key_from_addr(a));
    }
}

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

fn addr_for_key(t: &TorrentInner, key: PeerKey) -> Option<SocketAddr> {
    t.conns
        .read()
        .keys()
        .copied()
        .find(|&a| peer_key_from_addr(a) == key)
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

    fn chunk_dirty(&self, req: RequestIndex) -> bool {
        self.dirty_chunks.read().contains(&req)
    }

    fn mark_chunk_dirty(&self, req: RequestIndex) {
        self.dirty_chunks.write().insert(req);
        if let Some(book) = self.book.write().as_mut() {
            book.mark_chunk_dirty(req);
        }
        TorrentInner::bump_change(self);
    }

    fn clear_chunk_requested(&self, req: RequestIndex) {
        if let Some(book) = self.book.write().as_mut() {
            book.clear_chunk_requested(req);
        }
    }

    fn mark_chunk_requested(&self, req: RequestIndex) {
        if let Some(book) = self.book.write().as_mut() {
            book.mark_chunk_requested(req);
        }
    }

    fn piece_all_dirty(&self, piece: PieceIndex) -> bool {
        let pieces = self.pieces.read();
        let Some(p) = pieces.get(piece as usize) else {
            return false;
        };
        if p.chunks_total == 0 {
            return false;
        }
        let begin = self.piece_request_index_begin(piece);
        let dirty = self.dirty_chunks.read();
        (0..p.chunks_total as u32).all(|c| dirty.contains(&(begin + c)))
    }

    fn note_availability(&self, piece: PieceIndex, delta: i32) {
        if let Some(book) = self.book.write().as_mut() {
            for _ in 0..delta.unsigned_abs() {
                if delta > 0 { book.increment_relative_availability(piece); }
                else { book.decrement_relative_availability(piece); }
            }
        }
    }

    fn note_peer_piece(&self, peer: PeerKey, piece: PieceIndex, has: bool) {
        let mut peers = self.peer_pieces.write();
        let pieces = peers.entry(peer).or_default();
        if has { pieces.insert(piece); } else { pieces.remove(&piece); }
    }

    fn peer_closed(&self, addr: SocketAddr) {
        self.conn_handles.write().remove(&addr);
        self.peer_pieces.write().remove(&peer_key_from_addr(addr));
        self.bump_change();
    }

    fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()> {
        let info = self.info.read().clone().ok_or(CoreError::NoInfo)?;
        let storage = self
            .storage
            .read()
            .clone()
            .ok_or_else(|| CoreError::Other("no storage".into()))?;
        let piece_obj = info.piece(piece);
        let n = storage
            .piece(piece_obj, info.piece_hash_v1(piece))
            .write_at(data, i64::from(begin))?;
        if n != data.len() {
            return Err(CoreError::Other(format!(
                "short write: {n} of {}",
                data.len()
            )));
        }
        Ok(())
    }

    fn read_chunk(&self, piece: PieceIndex, begin: u32, length: u32) -> Result<Bytes> {
        let info = self.info.read().clone().ok_or(CoreError::NoInfo)?;
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
        let Some(info) = self.info.read().clone() else { return; };
        let Some(storage) = self.storage.read().clone() else { return; };
        let Some(hash) = info.piece_hash_v1(piece) else { return; };
        let priority = {
            let mut pieces = self.pieces.write();
            let Some(p) = pieces.get_mut(piece as usize) else { return; };
            if p.checking || p.complete { return; }
            p.checking = true;
            p.priority
        };
        let piece_obj = info.piece(piece);
        let job = crate::hashing::HashJob {
            piece_index: piece, priority, storage: storage.piece(piece_obj, Some(hash)),
            expected: crate::hashing::ExpectedHash::V1(hash), piece_length: info.piece_length,
            data_length: piece_obj.length, v1_zero_pad: 0, smartban: None,
            seq: self.hash_seq.fetch_add(1, Ordering::Relaxed),
        };
        if !self.hasher.lock().as_ref().is_some_and(|pool| pool.queue(job).is_ok()) {
            if let Some(p) = self.pieces.write().get_mut(piece as usize) { p.checking = false; }
        }
        TorrentInner::bump_change(self);
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
        self.assignments.write().remove(&req);
        self.request_state.write().remove(&req);
    }

    fn desired_request_context(&self) -> (RequestTorrentSnapshot, PieceRequestOrder) {
        let info_hash = self.info_hash.0;
        let piece_length = self.piece_length();
        let num_pieces = self.num_pieces();
        let focus_request = TorrentInner::request_focus_index(self);
        let pieces = self.pieces.read();
        let dirty = self.dirty_chunks.read();

        let mut piece_requestable = Vec::with_capacity(num_pieces as usize);
        let mut piece_count_unverified = Vec::with_capacity(num_pieces as usize);
        let mut undirtied_by_piece = Vec::with_capacity(num_pieces as usize);
        let mut piece_of_request = HashMap::new();
        let mut order = PieceRequestOrder::with_capacity(num_pieces as usize);

        for (i, p) in pieces.iter().enumerate() {
            let idx = i as PieceIndex;
            let requestable = !p.complete && !p.checking && p.priority > PiecePriority::None;
            piece_requestable.push(requestable);
            piece_count_unverified.push(p.checking || !p.chunks_received.is_empty());
            let begin = self.piece_request_index_begin(idx);
            let mut reqs = Vec::new();
            for c in 0..p.chunks_total as u32 {
                let r = begin + c;
                piece_of_request.insert(r, idx);
                if !dirty.contains(&r) {
                    reqs.push(r);
                }
            }
            undirtied_by_piece.push(reqs);
            order.add(
                PieceRequestOrderKey {
                    info_hash,
                    index: idx,
                },
                PieceRequestOrderState {
                    availability: 0,
                    priority: p.priority,
                    partial: p.partial,
                },
            );
        }
        drop(dirty);
        drop(pieces);

        let rs = self.request_state.read();
        let mut assignments = HashMap::new();
        for (r, a) in rs.iter() {
            let peer = peer_key_from_addr(a.peer);
            let outstanding = rs.values().filter(|x| x.peer == a.peer).count() as u64;
            assignments.insert(
                *r,
                PeerRequestAssignment {
                    peer,
                    when: a.when,
                    peer_outstanding: outstanding,
                },
            );
        }

        let snap = RequestTorrentSnapshot {
            info_hash,
            piece_length,
            num_pieces,
            piece_requestable,
            piece_count_unverified,
            undirtied_by_piece,
            piece_of_request,
            assignments,
            focus_request,
            has_active_webseed_requests: false,
            limits: StrategyLimits::default(),
        };
        (snap, order)
    }

    fn request_focus_index(&self) -> Option<RequestIndex> {
        TorrentInner::request_focus_index(self)
    }

    fn urgent_steal_after(&self) -> Duration {
        let t = current_seek_tuning();
        Duration::from_secs_f64((t.hol_after_ms / 1000.0).max(0.0))
    }

    fn seek_tuning(&self) -> SeekTuning {
        current_seek_tuning()
    }

    fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey> {
        if let Some(peer) = self.hedge_peers.read().get(&req) {
            return Some(*peer);
        }
        self.hedges
            .read()
            .get(&req)
            .map(|a| peer_key_from_addr(*a))
    }

    fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey) {
        self.hedge_peers.write().insert(req, peer);
        if let Some(addr) = addr_for_key(self, peer) {
            self.hedges.write().insert(req, addr);
        }
    }

    fn clear_hedge(&self, req: RequestIndex) {
        self.hedge_peers.write().remove(&req);
        self.hedges.write().remove(&req);
    }

    fn record_rescue(&self) {
        self.rescues.fetch_add(1, Ordering::Relaxed);
    }

    fn record_hedge(&self) {
        self.hedges_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn peer_has_piece(&self, peer: PeerKey, piece: PieceIndex) -> bool {
        if self.peer_pieces.read().get(&peer).is_some_and(|pieces| pieces.contains(&piece)) {
            return true;
        }
        let Some(addr) = addr_for_key(self, peer) else {
            return false;
        };
        let conns = self.conns.read();
        let Some(conn) = conns.get(&addr) else {
            return false;
        };
        let inner = conn.inner.read();
        inner.peer_sent_have_all || inner.peer_pieces.contains(&piece)
    }

    fn webtor_peer_snap(&self, peer: PeerKey) -> Option<WebtorPeerSnap> {
        let addr = addr_for_key(self, peer)?;
        let conns = self.conns.read();
        let conn = conns.get(&addr)?;
        let closed = conn.closed.load(Ordering::Relaxed);
        let inner = conn.inner.read();
        Some(WebtorPeerSnap {
            key: peer,
            download_rate_bps: inner.download_rate.rate(),
            rtt: inner.rtt.estimate,
            queued: inner.outstanding_requests.len() as u32,
            cancelled: inner.cancelled_requests.len() as u32,
            nominal_max: inner.max_requests.max(1) as u32,
            peer_choking: inner.peer_choking,
            closed,
            last_useful: inner.last_useful_chunk_received,
        })
    }

    fn connected_peer_keys(&self) -> Vec<PeerKey> {
        self.conns
            .read()
            .keys()
            .map(|&a| peer_key_from_addr(a))
            .collect()
    }

    fn on_metadata_data(&self, piece: u32, total_size: u32, data: Bytes) -> Result<bool> {
        let verified = {
            let mut metadata = self.metadata.write();
            metadata.set_size(total_size).map_err(|e| CoreError::Other(e.to_string()))?;
            metadata.on_data(piece, &data).map_err(|e| CoreError::Other(e.to_string()))?
        };
        if let Some(verified) = verified {
            self.set_info(Arc::new(verified.info))?;
            *self.metainfo_raw.write() = Some(wt_types::MetaInfo { info_bytes: verified.info_bytes, ..Default::default() });
            return Ok(true);
        }
        Ok(false)
    }

    fn metadata_piece(&self, piece: u32) -> Option<(u32, Bytes)> {
        let raw = self.metainfo_raw.read();
        let bytes = &raw.as_ref()?.info_bytes;
        let start = (piece as usize).checked_mul(crate::metadata::METADATA_PIECE_SIZE)?;
        if start >= bytes.len() { return None; }
        let end = (start + crate::metadata::METADATA_PIECE_SIZE).min(bytes.len());
        Some((bytes.len() as u32, Bytes::copy_from_slice(&bytes[start..end])))
    }

    fn metadata_size(&self) -> Option<u32> {
        self.metainfo_raw.read().as_ref().map(|m| m.info_bytes.len() as u32).or_else(|| self.metadata.read().size())
    }

    fn has_metadata(&self) -> bool {
        self.info.read().is_some()
    }

    fn ingest_pex(&self, _from: SocketAddr, _msg: &PexMsg) -> Result<()> {
        // TODO: PexState field
        Ok(())
    }

    fn add_dht_node(&self, addr: SocketAddr) {
        let mut nodes = self.dht_nodes.write();
        if !nodes.contains(&addr) { nodes.push(addr); }
    }

    fn on_peer_listen_port(&self, _peer_addr: SocketAddr, _port: u16) {}

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

    fn on_chunk_received(&self, peer: PeerKey, req: RequestIndex, len: usize) {
        self.clear_assignment(req);
        self.clear_hedge(req);
        if let Some(addr) = addr_for_key(self, peer) {
            if let Some(conn) = self.conns.read().get(&addr) {
                conn.note_block_received(req, Some(&mut *self.arrival_model.write()));
                let inner = conn.inner.read();
                inner
                    .stats
                    .bytes_read_data
                    .fetch_add(len as i64, Ordering::Relaxed);
                inner
                    .stats
                    .bytes_read_useful_data
                    .fetch_add(len as i64, Ordering::Relaxed);
                inner.stats.chunks_read.fetch_add(1, Ordering::Relaxed);
                inner
                    .stats
                    .chunks_read_useful
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        TorrentInner::bump_change(self);
    }
}

/// Placeholder until `book`/`glue` fields exist — call from `set_info` later.
pub fn init_book_and_glue(t: &TorrentInner, info: &wt_types::Info) {
    let mut book = t.book.write();
    if book.is_none() {
        *book = Some(crate::piece_state::PieceBook::new(info.num_pieces(), info.piece_length, info.total_length(), BLOCK_SIZE));
    }
}

/// Placeholder for Have/Bitfield → availability once `peer_pieces`/`book` exist.
pub fn note_peer_piece(t: &TorrentInner, peer: PeerKey, piece: PieceIndex, has: bool) {
    let mut peers = t.peer_pieces.write();
    let pieces = peers.entry(peer).or_default();
    let changed = if has { pieces.insert(piece) } else { pieces.remove(&piece) };
    drop(peers);
    if changed { t.note_availability(piece, if has { 1 } else { -1 }); }
}

impl TorrentInner {
    pub fn as_reader_host(self: &Arc<Self>) -> Arc<dyn crate::reader::ReaderHost> {
        Arc::clone(self) as Arc<dyn crate::reader::ReaderHost>
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

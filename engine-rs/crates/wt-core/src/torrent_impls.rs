//! `PeerTorrent` + `ReaderHost` for [`TorrentInner`].
//!
//! Backed by `PieceBook`, `StorageGlue`, hasher pool, strategy order, choking,
//! metadata/PEX, and change versioning.
//!
//! ## Required `pub(crate)` fields on `TorrentInner` (agy-2)
//!
//! ```ignore
//! book: RwLock<Option<PieceBook>>,
//! glue: RwLock<Option<StorageGlue>>,
//! reader_ranges: RwLock<HashMap<u64, PieceRange>>,
//! hasher: RwLock<Option<PieceHasherPool>>,
//! hash_seq: AtomicU64,
//! peer_key_by_addr: RwLock<HashMap<SocketAddr, PeerKey>>,
//! addr_by_peer_key: RwLock<HashMap<PeerKey, SocketAddr>>,
//! next_peer_key: AtomicU64, // reserved; keys are currently addr-derived
//! assignments: RwLock<HashMap<RequestIndex, (PeerKey, Instant)>>,
//! hedge_peers: RwLock<HashMap<RequestIndex, PeerKey>>,
//! peer_pieces: RwLock<HashMap<PeerKey, Bitset>>,
//! piece_request_order: RwLock<PieceRequestOrder>,
//! choke_config: RwLock<ChokeConfig>,
//! metadata: RwLock<MetadataExchange>,
//! pex: RwLock<PexState>,
//! pex_last_recv: RwLock<HashMap<SocketAddr, Instant>>,
//! dht_nodes: RwLock<Vec<SocketAddr>>,
//! peer_listen_ports: RwLock<HashMap<SocketAddr, u16>>,
//! ```
//!
//! ## Method coverage
//!
//! Fully implemented: info/flags, chunk dirty/requested, availability,
//! write/read chunk, queue_piece_hash (pool when present), assignments/hedges,
//! desired_request_context, focus/tuning/counters, peer_has_piece / webtor snap /
//! connected keys, metadata BEP 9, PEX ingest, DHT/listen-port lists, choke
//! config, bump_change, on_chunk_received, full ReaderHost.
//!
//! Still thin / TODO for callers: applying verified metadata via `set_info`,
//! pushing `dht_nodes` into `sources::Sources`, draining hash results into
//! mark_complete / pend_all_chunks (client/torrent loop).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use wt_types::{PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};
use wt_wire::ext::PexMsg;

use crate::choking::ChokeConfig;
use crate::hashing::{ExpectedHash, HashJob, SmartBanPieceRange};
use crate::metadata::Serve;
use crate::peer_conn::{PeerKey, PeerTorrent, WebtorPeerSnap};
use crate::picker::req_index_from_piece_and_offset;
use crate::piece_state::{Bitset, PieceBook};
use crate::reader::{reader_piece_priorities, PieceRange, ReaderHost};
use crate::storage_adapter::StorageGlue;
use crate::strategy::{
    PeerRequestAssignment, PieceRequestOrder, PieceRequestOrderKey, PieceRequestOrderState,
    RequestTorrentSnapshot, StrategyLimits,
};
use crate::torrent::TorrentInner;
use crate::tuning::{current_seek_tuning, SeekTuning};
use crate::{CoreError, Result};

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

fn apply_reader_ranges_to_book(book: &mut PieceBook, ranges: &HashMap<u64, PieceRange>) {
    let list: Vec<PieceRange> = ranges.values().copied().collect();
    let (now, readahead) = reader_piece_priorities(&list);
    let now_v: Vec<u32> = now.into_iter().collect();
    let ra_v: Vec<u32> = readahead.into_iter().collect();
    book.set_reader_now(&now_v);
    book.set_reader_readahead(&ra_v);
}

impl TorrentInner {
    pub(crate) fn ensure_peer_key(&self, addr: SocketAddr) -> PeerKey {
        if let Some(&k) = self.peer_key_by_addr.read().get(&addr) {
            return k;
        }
        let k = peer_key_from_addr(addr);
        self.peer_key_by_addr.write().insert(addr, k);
        self.addr_by_peer_key.write().insert(k, addr);
        let _ = self.next_peer_key.fetch_add(1, Ordering::Relaxed);
        k
    }

    fn addr_for_peer_key(&self, key: PeerKey) -> Option<SocketAddr> {
        if let Some(&a) = self.addr_by_peer_key.read().get(&key) {
            return Some(a);
        }
        self.conns
            .read()
            .keys()
            .copied()
            .find(|&a| peer_key_from_addr(a) == key)
    }

    fn refresh_piece_request_order(book: &PieceBook, ih: [u8; 20], order: &mut PieceRequestOrder) {
        let n = book.num_pieces();
        for i in 0..n {
            order.add(
                PieceRequestOrderKey {
                    info_hash: ih,
                    index: i,
                },
                PieceRequestOrderState {
                    availability: book.availability(i),
                    priority: book.effective_priority(i, true, false),
                    partial: book.piece_partially_downloaded(i),
                },
            );
        }
    }
}

impl PeerTorrent for TorrentInner {
    fn peer_key(&self, addr: SocketAddr) -> PeerKey {
        self.ensure_peer_key(addr)
    }

    fn have_info(&self) -> bool {
        self.info.read().is_some()
    }

    fn num_pieces(&self) -> u32 {
        if let Some(book) = self.book.read().as_ref() {
            return book.num_pieces();
        }
        self.info
            .read()
            .as_ref()
            .map(|i| i.num_pieces())
            .unwrap_or(0)
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
        self.seeding.load(Ordering::Relaxed)
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn piece_complete(&self, piece: PieceIndex) -> bool {
        self.book
            .read()
            .as_ref()
            .map(|b| b.piece_complete(piece))
            .unwrap_or(false)
    }

    fn piece_priority(&self, piece: PieceIndex) -> PiecePriority {
        self.book
            .read()
            .as_ref()
            .map(|b| {
                b.effective_priority(
                    piece,
                    true,
                    self.data_download_disallowed.load(Ordering::Relaxed),
                )
            })
            .unwrap_or(PiecePriority::None)
    }

    fn chunk_dirty(&self, req: RequestIndex) -> bool {
        if let Some(book) = self.book.read().as_ref() {
            return book.chunk_is_dirty(req);
        }
        self.dirty_chunks.read().contains(&req)
    }

    fn mark_chunk_dirty(&self, req: RequestIndex) {
        if let Some(book) = self.book.write().as_mut() {
            book.mark_chunk_dirty(req);
        }
        self.dirty_chunks.write().insert(req);
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
        self.book
            .read()
            .as_ref()
            .map(|b| b.piece_all_dirty(piece))
            .unwrap_or(false)
    }

    fn note_availability(&self, piece: PieceIndex, delta: i32) {
        let Some(book) = self.book.write().as_mut() else {
            return;
        };
        if delta > 0 {
            for _ in 0..delta {
                book.increment_relative_availability(piece);
            }
        } else {
            for _ in 0..(-delta) {
                book.decrement_relative_availability(piece);
            }
        }
    }

    fn write_chunk(&self, piece: PieceIndex, begin: u32, data: &[u8]) -> Result<()> {
        let info = self.info.read().clone().ok_or(CoreError::NoInfo)?;
        let piece_obj = info.piece(piece);
        let hash = info.piece_hash_v1(piece);
        if let Some(glue) = self.glue.read().as_ref() {
            glue.write_chunk(piece_obj, hash, i64::from(begin), data)?;
            return Ok(());
        }
        let storage = self
            .storage
            .read()
            .clone()
            .ok_or_else(|| CoreError::Other("no storage".into()))?;
        let n = storage
            .piece(piece_obj, hash)
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
        let piece_obj = info.piece(piece);
        let hash = info.piece_hash_v1(piece);
        let mut buf = vec![0u8; length as usize];
        let n = if let Some(glue) = self.glue.read().as_ref() {
            glue.read_at(piece_obj, hash, &mut buf, i64::from(begin))?
        } else {
            let storage = self
                .storage
                .read()
                .clone()
                .ok_or_else(|| CoreError::Other("no storage".into()))?;
            storage
                .piece(piece_obj, hash)
                .read_at(&mut buf, i64::from(begin))?
        };
        buf.truncate(n);
        Ok(Bytes::from(buf))
    }

    fn queue_piece_hash(&self, piece: PieceIndex) {
        if let Some(book) = self.book.write().as_mut() {
            book.queue_for_hash(piece);
        }
        let Some(info) = self.info.read().clone() else {
            return;
        };
        let Some(hash) = info.piece_hash_v1(piece) else {
            return;
        };
        let piece_obj = info.piece(piece);
        let storage = {
            if let Some(glue) = self.glue.read().as_ref() {
                glue.piece_storage(piece_obj, Some(hash))
            } else if let Some(s) = self.storage.read().as_ref() {
                s.piece(piece_obj, Some(hash))
            } else {
                return;
            }
        };
        let begin = self
            .book
            .read()
            .as_ref()
            .map(|b| b.piece_request_index_begin(piece))
            .unwrap_or_else(|| self.piece_request_index_begin(piece));
        let seq = self.hash_seq.fetch_add(1, Ordering::Relaxed);
        let prio = self.piece_priority(piece);
        let guard = self.hasher.read();
        let Some(pool) = guard.as_ref() else {
            return;
        };
        let _ = pool.queue(HashJob {
            piece_index: piece,
            priority: prio,
            storage,
            expected: ExpectedHash::V1(hash),
            piece_length: info.piece_length,
            data_length: piece_obj.length,
            v1_zero_pad: 0,
            smartban: Some(SmartBanPieceRange {
                block_begin: begin,
                chunk_size: BLOCK_SIZE,
            }),
            seq,
        });
    }

    fn requesting_peer(&self, req: RequestIndex) -> Option<PeerKey> {
        self.assignments
            .read()
            .get(&req)
            .map(|(k, _)| *k)
            .or_else(|| {
                self.request_state
                    .read()
                    .get(&req)
                    .map(|a| peer_key_from_addr(a.peer))
            })
    }

    fn request_when(&self, req: RequestIndex) -> Option<Instant> {
        self.assignments
            .read()
            .get(&req)
            .map(|(_, t)| *t)
            .or_else(|| self.request_state.read().get(&req).map(|a| a.when))
    }

    fn assign_request(&self, req: RequestIndex, peer: PeerKey, when: Instant) {
        self.assignments.write().insert(req, (peer, when));
        if let Some(addr) = self.addr_for_peer_key(peer) {
            self.request_state.write().insert(
                req,
                crate::picker::ActiveRequest {
                    peer: addr,
                    when,
                    cancelled: false,
                },
            );
        }
        self.mark_chunk_requested(req);
    }

    fn clear_assignment(&self, req: RequestIndex) {
        self.assignments.write().remove(&req);
        self.request_state.write().remove(&req);
        self.clear_chunk_requested(req);
    }

    fn desired_request_context(&self) -> (RequestTorrentSnapshot, PieceRequestOrder) {
        let info_hash = self.info_hash.0;
        let piece_length = self.piece_length();
        let num_pieces = self.num_pieces();
        let focus_request = <Self as PeerTorrent>::request_focus_index(self);
        let disallowed = self.data_download_disallowed.load(Ordering::Relaxed);

        let mut piece_requestable = vec![false; num_pieces as usize];
        let mut piece_count_unverified = vec![false; num_pieces as usize];
        let mut undirtied_by_piece = Vec::with_capacity(num_pieces as usize);
        let mut piece_of_request = HashMap::new();

        if let Some(book) = self.book.read().as_ref() {
            for p in 0..num_pieces {
                let ignore = book.ignore_for_requests(p, true, disallowed);
                piece_requestable[p as usize] =
                    !ignore && book.effective_priority(p, true, disallowed) > PiecePriority::None;
                piece_count_unverified[p as usize] =
                    book.queued_for_hash(p) || book.piece_has_dirty_chunks(p);
                let begin = book.piece_request_index_begin(p);
                let end = book.piece_request_index_end(p);
                let clean = book.iter_clean_chunk_indexes(p);
                let mut reqs = Vec::with_capacity(clean.len());
                for c in clean {
                    let r = begin + c;
                    piece_of_request.insert(r, p);
                    reqs.push(r);
                }
                for r in begin..end {
                    piece_of_request.entry(r).or_insert(p);
                }
                undirtied_by_piece.push(reqs);
            }
        } else {
            undirtied_by_piece.resize(num_pieces as usize, Vec::new());
        }

        let assign_snap = self.assignments.read().clone();
        let mut assignments = HashMap::new();
        for (r, (peer, when)) in &assign_snap {
            let outstanding = assign_snap.values().filter(|(k, _)| *k == *peer).count() as u64;
            assignments.insert(
                *r,
                PeerRequestAssignment {
                    peer: *peer,
                    when: *when,
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

        let mut order = self.piece_request_order.write();
        if let Some(book) = self.book.read().as_ref() {
            Self::refresh_piece_request_order(book, info_hash, &mut order);
        }
        let cloned = order.clone();
        (snap, cloned)
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
        self.hedge_peers
            .read()
            .get(&req)
            .copied()
            .or_else(|| self.hedges.read().get(&req).map(|a| peer_key_from_addr(*a)))
    }

    fn set_hedge_peer(&self, req: RequestIndex, peer: PeerKey) {
        self.hedge_peers.write().insert(req, peer);
        if let Some(addr) = self.addr_for_peer_key(peer) {
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
        if let Some(bits) = self.peer_pieces.read().get(&peer) {
            return bits.contains(piece);
        }
        let Some(addr) = self.addr_for_peer_key(peer) else {
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
        let addr = self.addr_for_peer_key(peer)?;
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
            .map(|&a| self.ensure_peer_key(a))
            .collect()
    }

    fn on_metadata_data(&self, piece: u32, total_size: u32, data: Bytes) -> Result<bool> {
        let mut meta = self.metadata.write();
        if meta.size().is_none() {
            meta.set_size(total_size)
                .map_err(|e| CoreError::Other(e.to_string()))?;
        }
        match meta.on_data(piece, data.as_ref()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(CoreError::Other(e.to_string())),
        }
    }

    fn metadata_piece(&self, piece: u32) -> Option<(u32, Bytes)> {
        let meta = self.metadata.read();
        let total = meta.size()?;
        match meta.serve(piece) {
            Serve::Data(b) => Some((total, b)),
            Serve::Reject => None,
        }
    }

    fn metadata_size(&self) -> Option<u32> {
        self.metadata.read().size()
    }

    fn has_metadata(&self) -> bool {
        self.metadata.read().has_info() || self.info.read().is_some()
    }

    fn ingest_pex(&self, from: SocketAddr, msg: &PexMsg) -> Result<()> {
        let now = Instant::now();
        let mut last_map = self.pex_last_recv.write();
        let mut last_opt = last_map.get(&from).copied();
        let peers = {
            let mut pex = self.pex.write();
            pex.ingest(msg, now, &mut last_opt)
                .map_err(|e| CoreError::Other(e.to_string()))?
        };
        if let Some(t) = last_opt {
            last_map.insert(from, t);
        }
        drop(last_map);
        let mut known = self.known_peers.write();
        for p in peers {
            known.insert(p);
        }
        Ok(())
    }

    fn add_dht_node(&self, addr: SocketAddr) {
        let mut nodes = self.dht_nodes.write();
        if !nodes.contains(&addr) {
            nodes.push(addr);
        }
    }

    fn on_peer_listen_port(&self, peer_addr: SocketAddr, port: u16) {
        self.peer_listen_ports.write().insert(peer_addr, port);
        self.ensure_peer_key(peer_addr);
    }

    fn choke_config(&self) -> ChokeConfig {
        let mut cfg = self.choke_config.read().clone();
        cfg.data_upload_disallowed = self.data_upload_disallowed.load(Ordering::Relaxed);
        cfg.seeding = self.seeding.load(Ordering::Relaxed);
        cfg
    }

    fn bump_change(&self) {
        TorrentInner::bump_change(self);
    }

    fn on_chunk_received(&self, peer: PeerKey, req: RequestIndex, len: usize) {
        self.clear_assignment(req);
        self.clear_hedge(req);
        if let Some(addr) = self.addr_for_peer_key(peer) {
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

impl ReaderHost for TorrentInner {
    fn have(&self, offset: i64, len: i64, responsive: bool) -> i64 {
        let Some(info) = self.info.read().clone() else {
            return 0;
        };
        let piece_len = info.piece_length;
        if piece_len <= 0 {
            return 0;
        }
        let total = info.total_length();
        let mut off = offset;
        let mut max = len;
        let mut ret = 0i64;
        let book_guard = self.book.read();
        let book = book_guard.as_ref();

        while max > 0 {
            if off < 0 || off >= total {
                break;
            }
            let piece = (off / piece_len) as PieceIndex;
            let in_piece = off % piece_len;
            let chunk_start = (in_piece / i64::from(BLOCK_SIZE)) * i64::from(BLOCK_SIZE);
            let piece_end = ((i64::from(piece) + 1) * piece_len).min(total);
            let piece_base = i64::from(piece) * piece_len;
            let chunk_len = (piece_end - (piece_base + chunk_start)).min(i64::from(BLOCK_SIZE));
            if chunk_len <= 0 {
                break;
            }

            let complete = book.map(|b| b.piece_complete(piece)).unwrap_or(false);
            if !responsive && !complete {
                break;
            }
            if !complete {
                let req = req_index_from_piece_and_offset(piece, chunk_start as u32, piece_len);
                let dirty = book
                    .map(|b| b.chunk_is_dirty(req))
                    .unwrap_or_else(|| self.dirty_chunks.read().contains(&req));
                if !dirty {
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
        let deadline = Instant::now() + timeout;
        let mut guard = self.change_mutex.lock();
        loop {
            let cur = self.change_version.load(Ordering::SeqCst);
            if cur > after || Instant::now() >= deadline {
                return cur;
            }
            let wait_dur = deadline.saturating_duration_since(Instant::now());
            if wait_dur.is_zero() {
                return cur;
            }
            self.change_cond.wait_for(&mut guard, wait_dur);
        }
    }

    fn change_version(&self) -> u64 {
        self.change_version.load(Ordering::SeqCst)
    }

    fn set_reader_positions(&self, reader_id: u64, range: PieceRange) {
        self.reader_ranges.write().insert(reader_id, range);
        {
            let ranges = self.reader_ranges.read().clone();
            if let Some(book) = self.book.write().as_mut() {
                apply_reader_ranges_to_book(book, &ranges);
            }
        }
        if !range.is_empty() {
            if let Some(piece_len) = self.info.read().as_ref().map(|i| i.piece_length) {
                let focus_off = i64::from(range.begin) * piece_len;
                self.request_focus.store(focus_off + 1, Ordering::Relaxed);
            }
        }
        TorrentInner::bump_change(self);
    }

    fn clear_reader(&self, reader_id: u64) {
        self.reader_ranges.write().remove(&reader_id);
        {
            let ranges = self.reader_ranges.read().clone();
            if let Some(book) = self.book.write().as_mut() {
                apply_reader_ranges_to_book(book, &ranges);
            }
        }
        TorrentInner::bump_change(self);
    }

    fn read_at(&self, buf: &mut [u8], torrent_offset: i64) -> std::io::Result<usize> {
        let info = self.info.read().clone().ok_or_else(|| {
            std::io::Error::new(ErrorKind::NotConnected, "torrent info not available")
        })?;
        let piece_len = info.piece_length;
        if piece_len <= 0 || torrent_offset < 0 || torrent_offset >= info.total_length() {
            return Ok(0);
        }
        let piece_idx = (torrent_offset / piece_len) as PieceIndex;
        let in_piece = torrent_offset % piece_len;
        let piece_obj = info.piece(piece_idx);
        let rem = (piece_obj.length - in_piece).max(0) as usize;
        if rem == 0 || buf.is_empty() {
            return Ok(0);
        }
        let to_read = buf.len().min(rem);
        let hash = info.piece_hash_v1(piece_idx);
        if let Some(glue) = self.glue.read().as_ref() {
            return glue
                .read_at(piece_obj, hash, &mut buf[..to_read], in_piece)
                .map_err(|e| std::io::Error::other(e.to_string()));
        }
        let storage = self.storage.read().clone().ok_or_else(|| {
            std::io::Error::new(ErrorKind::NotConnected, "torrent storage not available")
        })?;
        storage
            .piece(piece_obj, hash)
            .read_at(&mut buf[..to_read], in_piece)
            .map_err(|e| std::io::Error::other(e.to_string()))
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
        PeerTorrent::num_pieces(self)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn has_info(&self) -> bool {
        self.info.read().is_some()
    }

    fn on_read_waiting(&self, torrent_offset: i64) {
        self.request_focus
            .store(torrent_offset + 1, Ordering::Relaxed);
        self.nudge_urgent_requests();
    }
}

/// Record a peer's Have/Bitfield bit and adjust availability.
pub fn note_peer_piece(t: &TorrentInner, peer: PeerKey, piece: PieceIndex, has: bool) {
    let mut map = t.peer_pieces.write();
    let bits = map.entry(peer).or_insert_with(Bitset::new);
    if has {
        bits.insert(piece);
    } else {
        bits.remove(piece);
    }
    drop(map);
    if let Some(book) = t.book.write().as_mut() {
        if has {
            book.increment_relative_availability(piece);
        } else {
            book.decrement_relative_availability(piece);
        }
    }
}

/// Hook for agy-2 `set_info`: create `PieceBook` + `StorageGlue`.
pub fn init_book_and_glue(t: &TorrentInner, info: &wt_types::Info) {
    if let Some(storage) = t.storage.read().clone() {
        *t.glue.write() = Some(StorageGlue::new(storage));
    }
    *t.book.write() = Some(PieceBook::new(
        info.num_pieces(),
        info.piece_length,
        info.total_length(),
        BLOCK_SIZE,
    ));
}

/// Expose Arc for type-erased ReaderHost construction.
impl TorrentInner {
    pub fn as_reader_host(self: &Arc<Self>) -> Arc<dyn ReaderHost> {
        Arc::clone(self) as Arc<dyn ReaderHost>
    }
}

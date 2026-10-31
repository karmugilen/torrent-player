//! `PeerTorrent` for [`TorrentInner`], and its `ReaderHost`.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use wt_types::{PieceIndex, PiecePriority, RequestIndex, BLOCK_SIZE};
use wt_wire::ext::PexMsg;

use crate::choking::ChokeConfig;
use crate::peer_conn::{PeerKey, PeerTorrent};
use crate::picker::req_index_from_piece_and_offset;
use crate::reader::{reader_piece_priorities, PieceRange, ReaderHost};
use crate::torrent::TorrentInner;
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
        if self.dirty_chunks.contains(req) || !self.blocks.begin_write(req) {
            return false;
        }
        // The writer may have finished between the two checks above.
        if self.dirty_chunks.contains(req) {
            self.blocks.end_write(req);
            return false;
        }
        true
    }

    fn finish_chunk_write(&self, req: RequestIndex, written: bool) {
        // Dirty before releasing the reservation: a concurrent duplicate sees
        // one state or the other, never neither.
        if written {
            self.mark_chunk_dirty(req);
        }
        self.blocks.end_write(req);
    }

    fn any_wanted_incomplete(&self) -> bool {
        self.wanted_pieces().incomplete > 0
    }

    fn any_wanted_among(&self, wanted: &crate::piece_state::Bitset) -> bool {
        let pieces = self.pieces.read();
        wanted.iter().any(|i| {
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

    fn pick_blocks(&self, peer: &crate::sched::PickPeer<'_>, want: usize) -> crate::sched::Pick {
        TorrentInner::pick_blocks(self, peer, want)
    }

    fn register_live(&self, live: Arc<crate::sched::PeerLive>) {
        self.live_peers.write().push(live);
    }

    fn unregister_live(&self, live: &Arc<crate::sched::PeerLive>) {
        self.live_peers.write().retain(|other| !Arc::ptr_eq(other, live));
    }

    fn focus_block_claimed(&self) {
        if !self.seek_hedge.lock()
            .is_some_and(|(at, left)| left > 0 && at.elapsed() < crate::torrent::SEEK_HEDGE_WINDOW)
        {
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


    fn displaced_requests(
        &self,
        requests: &std::collections::HashSet<RequestIndex>,
        peer: PeerKey,
    ) -> Vec<RequestIndex> {
        // A block someone else delivered is displaced too: cancelling it saves
        // the duplicate on its way.
        requests
            .iter()
            .copied()
            .filter(|&req| self.dirty_chunks.contains(req) || !self.blocks.holds(req, peer))
            .collect()
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
        self.blocks.owner(req).map(|(peer, _)| peer)
    }


    fn request_when(&self, req: RequestIndex) -> Option<Instant> {
        self.blocks.owner(req).map(|(_, when)| when)
    }



    fn clear_assignment(&self, req: RequestIndex) {
        self.blocks.clear_owner(req);
    }


    fn claim_request(&self, req: RequestIndex, peer: PeerKey, when: Instant) -> bool {
        !self.blocks.is_writing(req)
            && !self.dirty_chunks.contains(req)
            && self.blocks.claim(req, peer, when)
    }


    fn release_request(&self, req: RequestIndex, peer: PeerKey) {
        self.blocks.release(req, peer);
    }




    fn request_focus_index(&self) -> Option<RequestIndex> {
        TorrentInner::request_focus_index(self)
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


    fn refund_seek_hedge(&self) {
        if let Some((_, left)) = &mut *self.seek_hedge.lock() {
            *left += 1;
        }
    }



    fn hedge_peer(&self, req: RequestIndex) -> Option<PeerKey> {
        self.blocks.second(req)
    }



    fn claim_hedge(&self, req: RequestIndex, peer: PeerKey) -> bool {
        !self.dirty_chunks.contains(req) && self.blocks.claim_second(req, peer)
    }


    fn clear_hedge(&self, req: RequestIndex) {
        self.blocks.clear_second(req);
    }


    fn release_hedge(&self, req: RequestIndex, peer: PeerKey) {
        self.blocks.release_second(req, peer);
    }


    fn record_rescue(&self) {
        self.rescues.fetch_add(1, Ordering::Relaxed);
    }

    fn record_hedge(&self) {
        self.hedges_sent.fetch_add(1, Ordering::Relaxed);
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
        self.blocks.clear(req);
    }

}

/// Size the availability counters, counting bitfields that arrived before info.
pub(crate) fn init_availability(t: &TorrentInner, num_pieces: u32) {
    let counts = t.piece_availability.get_or_init(|| {
        (0..num_pieces).map(|_| std::sync::atomic::AtomicI32::new(0)).collect()
    });
    for peer in t.live_peers.read().iter() {
        for piece in peer.pieces.read().iter() {
            if let Some(count) = counts.get(piece as usize) {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}


impl TorrentInner {

}

impl TorrentInner {
    /// Rewrites `pieces` only if a priority changes, since that rescans them all.
    fn apply_reader_priorities(&self) {
        let ranges: Vec<PieceRange> = self.reader_ranges.read().values().copied().collect();
        let (now, readahead) = reader_piece_priorities(&ranges);
        let target = |piece: &crate::torrent::PieceEntry| {
            piece.base_priority.max(if now.contains(&piece.index) {
                PiecePriority::Now
            } else if readahead.contains(&piece.index) {
                PiecePriority::High
            } else {
                PiecePriority::None
            })
        };
        if self.pieces.read().iter().all(|piece| piece.priority == target(piece)) {
            return;
        }
        for piece in self.pieces_mut().iter_mut() {
            piece.priority = target(piece);
        }
    }

    pub fn peer_has_piece(&self, peer: PeerKey, piece: PieceIndex) -> bool {
        self.live_peers.read().iter().any(|p| p.key == peer && p.pieces.read().contains(piece))
    }

    pub fn peer_piece_count(&self, peer: PeerKey) -> u64 {
        self.live_peers.read().iter().find(|p| p.key == peer).map_or(0, |p| p.pieces.read().len())
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
        // Called on every read. Readers need waking; peers only need to hear
        // about a range that moved, and most reads do not move it.
        if self.reader_ranges.write().insert(reader_id, range) == Some(range) {
            self.notify_progress();
            return;
        }
        self.apply_reader_priorities();
        self.bump_change();
        self.nudge_urgent_requests();
    }

    fn clear_reader(&self, reader_id: u64) {
        self.reader_ranges.write().remove(&reader_id);
        self.apply_reader_priorities();
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

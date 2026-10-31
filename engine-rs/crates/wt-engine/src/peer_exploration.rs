//! Engine sampling and guarded release of long-idle peer connections.
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Instant;

use parking_lot::Mutex;
use wt_policy::peer_exploration::{PeerExploration, PeerExplorationObservation};

use crate::server::{EngineServer, TorrentRecord};

#[derive(Default)]
pub struct ExplorationState(pub(crate) Mutex<HashMap<String, PeerExploration>>);

pub(crate) fn eligible(server: &EngineServer, rec: &TorrentRecord) -> bool {
    server.payload_transfers_allowed.load(Ordering::SeqCst)
        && rec.configured
        && !rec.paused
        && !rec.checking
        && !rec.torrent.closed()
        && rec.error.is_none()
        && !rec.seeding
        && !rec.selected.is_empty()
}

// Use the live connection identity rather than its endpoint, which can reconnect.
pub(crate) fn peer_identity(torrent: &wt_core::Torrent, addr: std::net::SocketAddr) -> u64 {
    if let Some(handle) = torrent.inner.conn_handles.read().get(&addr) {
        return std::sync::Arc::as_ptr(&handle.stats()) as usize as u64;
    }
    wt_core::torrent_impls::peer_key_from_addr(addr)
}

/// Read current atomic counters even if the public connection snapshot is older.
pub(crate) fn live_peers(torrent: &wt_core::Torrent) -> Vec<wt_core::PeerConnInfo> {
    let mut peers = torrent.peer_conns();
    let handles = torrent.inner.conn_handles.read();
    peers.retain_mut(|peer| {
        if let Some(handle) = handles.get(&peer.remote_addr) {
            if handle.is_closed() {
                return false;
            }
            peer.conn_stats = handle.stats().snapshot();
        }
        true
    });
    peers
}

/// Recheck live transfer counters and identity immediately before closing a slot.
pub(crate) fn release_unchanged(
    torrent: &wt_core::Torrent,
    peer: &wt_core::PeerConnInfo,
    identity: u64,
    protect_seed: bool,
) -> bool {
    if peer_identity(torrent, peer.remote_addr) != identity {
        return false;
    }
    let Some(latest) = live_peers(torrent)
        .into_iter()
        .find(|p| p.remote_addr == peer.remote_addr)
    else {
        return false;
    };
    let bitfield_key = wt_core::torrent_impls::peer_key_from_addr(peer.remote_addr);
    let is_seed = latest.peer_completion >= 1.0
        || (torrent.num_pieces() > 0
            && torrent.inner.peer_piece_count(bitfield_key) >= u64::from(torrent.num_pieces()));
    if latest.conn_stats.bytes_read_useful_data != peer.conn_stats.bytes_read_useful_data
        || latest.conn_stats.bytes_written_data != peer.conn_stats.bytes_written_data
        || (protect_seed && is_seed)
    {
        return false;
    }
    torrent.drop_peer(peer.remote_addr)
}

impl EngineServer {
    pub fn peer_exploration_tick(&self, now: Instant) {
        let records: Vec<_> = self.records.read().values().cloned().collect();
        let mut states = self.exploration_state.0.lock();
        if !self.peer_exploration.load(Ordering::SeqCst) {
            states.clear();
            return;
        }
        states.retain(|id, _| records.iter().any(|r| r.read().id == *id));
        for record in records {
            let rec = record.read();
            if !eligible(self, &rec) {
                states.remove(&rec.id);
                continue;
            }
            let peers = live_peers(&rec.torrent);
            let seeders = peers.iter().filter(|p| p.peer_completion >= 1.0).count();
            let observations: Vec<_> = peers
                .iter()
                .map(|peer| PeerExplorationObservation {
                    peer: peer_identity(&rec.torrent, peer.remote_addr),
                    useful: peer.conn_stats.bytes_read_useful_data,
                    uploaded: peer.conn_stats.bytes_written_data,
                    protected: seeders <= 1 && peer.peer_completion >= 1.0,
                })
                .collect();
            let victim = states.entry(rec.id.clone()).or_default().choose(
                now,
                &observations,
                self.max_peers.load(Ordering::SeqCst),
                rec.torrent.stats().pending_peers,
            );
            if let Some(victim) = victim {
                if !self.peer_exploration.load(Ordering::SeqCst) || !eligible(self, &rec) {
                    continue;
                }
                if let Some(peer) = peers
                    .iter()
                    .zip(&observations)
                    .find_map(|(p, o)| (o.peer == victim).then_some(p))
                {
                    if release_unchanged(&rec.torrent, peer, victim, seeders <= 1) {
                        self.notify_change();
                    }
                }
            }
        }
    }
}

//! Ghost dial feedback and Ghost Pro bitfield-aware peer recycling.
use std::collections::HashMap;
use std::sync::{atomic::Ordering, Arc};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wt_policy::ghost_mode::{GhostDialController, GhostObservation};
use wt_policy::ghost_pro::{
    compute_wanted_pieces, contiguous_wanted_pieces, peer_overlap, GhostProPeerObservation,
    GhostProSwarm, WantedContext, WantedFile,
};
use wt_policy::piece_span::FileLayout;

use crate::peer_exploration::{eligible, live_peers, peer_identity, release_unchanged};
use crate::server::EngineServer;

#[derive(Default)]
pub struct GhostState {
    pub(crate) inner: Mutex<GhostInner>,
}
#[derive(Default)]
pub(crate) struct GhostInner {
    generation: u64,
    dial: GhostDialController,
    pro_dial: GhostDialController,
    swarms: HashMap<String, GhostProSwarm>,
}

impl EngineServer {
    /// Invalidates any observation collected before a settings/route change.
    pub fn reset_ghost(&self) {
        let mut state = self.ghost_state.inner.lock();
        state.generation = state.generation.wrapping_add(1);
        state.dial = GhostDialController::default();
        state.pro_dial = GhostDialController::default();
        state.swarms.clear();
        let base = self.dial_rate_per_sec.load(Ordering::SeqCst);
        self.client.inner.dial_limiter.set_limit(base as f64);
        self.client.inner.dial_limiter.set_burst(base);
    }

    pub fn ghost_mode_tick(&self, now: Instant) {
        self.sample_ghost(now, false);
    }
    pub fn ghost_pro_mode_tick(&self, now: Instant) {
        self.sample_ghost(now, true);
    }

    /// Feed an explicit sample through the same controller and limiter as the sampler.
    /// Useful for deterministic clocks, diagnostics and policy replay.
    pub fn sample_ghost_observation(&self, now: Instant, mut observation: GhostObservation) -> i32 {
        let mut state = self.ghost_state.inner.lock();
        let pro = self.ghost_pro_mode.load(Ordering::SeqCst);
        if !pro && !self.ghost_mode.load(Ordering::SeqCst) {
            return self.dial_rate_per_sec.load(Ordering::SeqCst) as i32;
        }
        observation.opportunity &= self.payload_transfers_allowed.load(Ordering::SeqCst);
        self.apply_ghost_observation(&mut state, now, observation, pro)
    }

    fn apply_ghost_observation(
        &self,
        state: &mut GhostInner,
        now: Instant,
        observation: GhostObservation,
        pro: bool,
    ) -> i32 {
        let base = self.dial_rate_per_sec.load(Ordering::SeqCst) as i32;
        let controller = if pro {
            &mut state.pro_dial
        } else {
            &mut state.dial
        };
        let rate = controller.observe(now, observation, base);
        self.client.inner.dial_limiter.set_limit(rate as f64);
        self.client.inner.dial_limiter.set_burst(rate as usize);
        rate
    }

    fn sample_ghost(&self, now: Instant, pro: bool) {
        let enabled = || {
            if pro {
                self.ghost_pro_mode.load(Ordering::SeqCst)
            } else {
                self.ghost_mode.load(Ordering::SeqCst)
            }
        };
        if !enabled() {
            return;
        }
        let generation = self.ghost_state.inner.lock().generation;
        let capacity = self.max_peers.load(Ordering::SeqCst);
        let records: Vec<_> = self.records.read().values().cloned().collect();
        let mut observation = GhostObservation::default();
        let mut cohort = Vec::new();
        for record in &records {
            let rec = record.read();
            if !eligible(self, &rec) {
                if pro {
                    self.ghost_state.inner.lock().swarms.remove(&rec.id);
                }
                continue;
            }
            let stats = rec.torrent.stats();
            if stats.pieces_complete < rec.torrent.num_pieces() as usize {
                cohort.push(format!("{}:{:?}", rec.id, rec.selected));
                observation.useful_bytes += stats.conn_stats.bytes_read_useful_data;
                observation.opportunity |= stats.pending_peers > 0 && stats.active_peers < capacity;
            }
            if !pro {
                continue;
            }
            let files = rec
                .torrent
                .files()
                .into_iter()
                .map(|f| WantedFile {
                    index: f.index(),
                    selected: rec.selected.contains(&f.index()),
                    layout: FileLayout::for_file(f.offset(), f.length(), rec.piece_length),
                })
                .collect::<Vec<_>>();
            let wanted = compute_wanted_pieces(
                &files,
                &WantedContext {
                    direct_play: rec.direct_play,
                    file_only: rec.file_only,
                    focused_file: rec.focused_file,
                    piece_length: rec.piece_length,
                    num_pieces: rec.torrent.num_pieces(),
                },
                |p| rec.torrent.piece_complete(p),
            );
            if wanted.is_empty() {
                continue;
            }
            let windows = self.playback_windows(&rec.id);
            let contiguous = contiguous_wanted_pieces(&windows, &wanted);
            let peers = live_peers(&rec.torrent);
            let bitfields = rec.torrent.inner.peer_pieces.read();
            let observations = peers
                .iter()
                .map(|peer| {
                    let key = wt_core::torrent_impls::peer_key_from_addr(peer.remote_addr);
                    let has = |p| {
                        peer.peer_completion >= 1.0
                            || bitfields.get(&key).is_some_and(|b| b.contains(&p))
                    };
                    GhostProPeerObservation {
                        peer: peer_identity(&rec.torrent, peer.remote_addr),
                        useful: peer.conn_stats.bytes_read_useful_data,
                        uploaded: peer.conn_stats.bytes_written_data,
                        overlap_pieces: peer_overlap(&wanted, has),
                        contiguous_overlap: peer_overlap(&contiguous, has),
                        is_seeder: peer.peer_completion >= 1.0,
                    }
                })
                .collect::<Vec<_>>();
            drop(bitfields);
            let mut state = self.ghost_state.inner.lock();
            if generation != state.generation || !enabled() {
                return;
            }
            let victim = state
                .swarms
                .entry(rec.id.clone())
                .or_default()
                .choose_victim(
                    now,
                    &observations,
                    capacity,
                    stats.pending_peers,
                    !contiguous.is_empty(),
                );
            if let Some(victim) = victim {
                if enabled() && eligible(self, &rec) {
                    if let Some(peer) = peers
                        .iter()
                        .zip(&observations)
                        .find_map(|(p, o)| (o.peer == victim).then_some(p))
                    {
                        if release_unchanged(&rec.torrent, peer, victim, true) {
                            self.notify_change();
                        }
                    }
                }
            }
        }
        cohort.sort();
        observation.cohort = cohort.join(",");
        observation.download_limit = self.download_rate_limit.load(Ordering::SeqCst);
        let mut state = self.ghost_state.inner.lock();
        if generation != state.generation || !enabled() {
            return;
        }
        observation.opportunity &= self.payload_transfers_allowed.load(Ordering::SeqCst);

        self.apply_ghost_observation(&mut state, now, observation, pro);
    }

    /// One weakly-owned task shares the existing one-second telemetry clock.
    pub(crate) fn start_policy_sampler(server: &Arc<Self>) {
        let weak = Arc::downgrade(server);
        server.client.inner.tokio_handle.spawn(async move {
            let mut previous = None;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(server) = weak.upgrade() else {
                    break;
                };
                if server.shutting_down.load(Ordering::SeqCst) {
                    break;
                }
                let now = Instant::now();
                server.sample_transfer_rates(now);
                server.ghost_mode_tick(now);
                server.ghost_pro_mode_tick(now);
                server.peer_exploration_tick(now);
                server.seed_tick();
                server.direct_play_idle_tick(now);
                server.park_idle_readers();
                // Go `dropChokersLoop`: free slots held by peers that keep us
                // choked while we are interested.
                if server.drop_chokers.load(Ordering::SeqCst) {
                    let after = wt_policy::choke_timeout();
                    for rec in server.records.read().values() {
                        let rec = rec.read();
                        if !rec.resting {
                            let _ = rec.torrent.drop_chokers(after);
                        }
                    }
                }
                server.refresh_torrent_trackers();
                let fingerprint = server.status_fingerprint();
                if previous.is_some_and(|last| last != fingerprint) {
                    server.notify_change();
                }
                previous = Some(fingerprint);
            }
        });
    }
}

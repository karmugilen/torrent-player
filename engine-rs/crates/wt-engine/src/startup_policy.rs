//! Startup hints, next-episode warm-up and idle Direct Play.
//!
//! Engine wiring for `engine-go/video_priority.go`, `next_preload.go` and
//! `direct_play_idle.go`. The decisions themselves live in `wt-policy`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wt_policy::{
    video_startup_hints, FileLayout, IdleAction, IdleDirectPlay, IdleTickInput, PreloadFile,
    VideoFileInput, VideoHintContext,
};

use crate::server::{EngineServer, TorrentRecord};
use crate::types::NextPreloadView;

/// Go `restPeerLimit`: dial addresses kept while a session rests.
const REST_PEER_LIMIT: usize = 80;

/// Per-torrent idle Direct Play state. Kept outside the record lock: reads
/// update it, and pause/remove may hold the record while cancelling a read.
#[derive(Default)]
pub(crate) struct IdleRecord {
    policy: IdleDirectPlay,
    rest_peers: Vec<wt_core::PeerInfo>,
}

impl EngineServer {
    fn file_layouts(rec: &TorrentRecord) -> Vec<(usize, String, FileLayout)> {
        rec.torrent
            .files()
            .into_iter()
            .map(|f| {
                (f.index(), f.path(), FileLayout::for_file(f.offset(), f.length(), rec.piece_length))
            })
            .collect()
    }

    /// Go `directStreamStorage`: RAM stream storage of a Direct Play record.
    fn direct_stream_storage(&self, rec: &TorrentRecord) -> Option<Arc<wt_storage::DocumentTorrentStorage>> {
        if !rec.direct_play {
            return None;
        }
        self.storage.get_storage(&rec.info_hash).filter(|st| st.stream_mode())
    }

    fn open_reader_count(&self, id: &str) -> usize {
        self.playback_readers.lock().values().filter(|r| r.id == id && !r.is_closed()).count()
    }

    /// Go `applyVideoStartupHints`: selected videos' head and tail pieces at
    /// Now so container metadata (e.g. moov atoms) arrives first, Direct Play
    /// startup-cache pieces at Normal and active next-episode pieces at High.
    /// Returns whether the hints changed. Caller holds the record lock.
    pub(crate) fn apply_video_startup_hints(&self, rec: &mut TorrentRecord) -> bool {
        let storage = self.direct_stream_storage(rec);
        let startup_fetch_pieces = storage
            .map(|st| st.startup_fetch_pieces(self.open_reader_count(&rec.id) > 0)
                .into_iter().map(|p| p as u32).collect())
            .unwrap_or_default();
        let ctx = VideoHintContext {
            transfers_allowed: self.payload_transfers_allowed.load(Ordering::SeqCst),
            paused: rec.paused,
            configured: rec.configured,
            checking: rec.checking,
            removed: rec.torrent.closed(),
            has_error: rec.error.is_some(),
            direct_play: rec.direct_play,
            file_only: rec.file_only,
            focused_file: rec.focused_file,
            selected: rec.selected.iter().copied().collect(),
            piece_length: rec.piece_length,
            ghost_pro: self.ghost_pro_mode.load(Ordering::SeqCst),
            next_preload_pieces: if rec.preload_plan.is_active() {
                rec.preload_plan.pieces().iter().copied().collect()
            } else {
                Vec::new()
            },
            startup_fetch_pieces,
        };
        let files: Vec<VideoFileInput> = Self::file_layouts(rec)
            .into_iter()
            .map(|(index, display_path, layout)| VideoFileInput { index, display_path, layout })
            .collect();
        let hints = video_startup_hints(&files, &ctx);
        if hints == rec.playback_coordinator.hints() {
            return false;
        }
        rec.playback_coordinator.set_hints(hints);
        true
    }

    /// Go `applyBackgroundFocus`: in download mode the focused (playing)
    /// file's pieces are High, ahead of other selected files.
    pub(crate) fn apply_background_focus(&self, rec: &mut TorrentRecord) {
        let active = self.payload_transfers_allowed.load(Ordering::SeqCst)
            && !rec.paused && rec.configured && !rec.checking;
        let mut demand = HashMap::new();
        if !rec.direct_play && active && rec.piece_length > 0 {
            if let Some(file) = rec.focused_file.and_then(|i| rec.torrent.files().into_iter().find(|f| f.index() == i)) {
                let layout = FileLayout::for_file(file.offset(), file.length(), rec.piece_length);
                for p in layout.begin_piece..layout.end_piece {
                    demand.insert(p, wt_core::PiecePriority::High);
                }
            }
        }
        rec.playback_coordinator.set_focus(demand);
    }

    /// Recompute hints and publish them if they changed (Go `refreshStartupHints`).
    pub(crate) fn refresh_startup_hints(&self, rec: &mut TorrentRecord) {
        if rec.torrent.info().is_none() || !self.apply_video_startup_hints(rec) {
            return;
        }
        let active = self.payload_transfers_allowed.load(Ordering::SeqCst)
            && rec.configured && !rec.paused && !rec.checking && rec.error.is_none();
        if active {
            let demand = rec.playback_coordinator.effective();
            Self::apply_record_piece_priorities(rec, &demand);
        }
    }

    /// Go `setNextPreloadLocked`: replace the warm-up plan for a new /play.
    pub(crate) fn set_next_preload(&self, rec: &mut TorrentRecord, current: usize, next: Option<usize>) {
        if rec.preload_plan.is_active() {
            if let Some(st) = self.direct_stream_storage(rec) {
                st.set_preload_file(usize::MAX);
            }
        }
        let files: Vec<PreloadFile> = Self::file_layouts(rec)
            .into_iter()
            .map(|(index, display_path, layout)| PreloadFile {
                index,
                display_path,
                layout,
                selected: rec.selected.contains(&index),
            })
            .collect();
        rec.preload_plan.set_plan(current, next, &files);
    }

    /// Go `startNextPreload`: warm the next episode once playback of
    /// `current` nears its end. Runs at most once per /play.
    pub(crate) fn start_next_preload(&self, id: &str, current: usize) {
        let Some(rec_arc) = self.record_by_id(id) else { return; };
        let mut rec = rec_arc.write();
        let plan = &rec.preload_plan;
        if plan.is_active() || plan.planned_next().is_none()
            || rec.torrent.closed() || rec.paused || !rec.configured || rec.error.is_some()
        {
            return;
        }
        let gate_current = rec.focused_file;
        if gate_current != Some(current) {
            return;
        }
        let Some(next) = plan.planned_next() else { return; };
        let Some((index, display_path, layout)) =
            Self::file_layouts(&rec).into_iter().find(|(i, _, _)| *i == next)
        else {
            return;
        };
        let file = PreloadFile { index, display_path, layout, selected: true };
        // Direct Play keeps only pieces its startup cache can hold.
        let overrides = self.direct_stream_storage(&rec).map(|st| {
            st.set_preload_file(next).into_keys().map(|p| p as u32).collect::<HashSet<_>>()
        });
        let (piece_length, direct) = (rec.piece_length, rec.direct_play);
        rec.preload_plan.activate(&file, piece_length, direct, overrides);
        self.refresh_startup_hints(&mut rec);
        drop(rec);
        self.notify_change();
    }

    /// Go `nextPreloadView`.
    pub(crate) fn next_preload_view(rec: &TorrentRecord) -> Option<NextPreloadView> {
        rec.preload_plan
            .view(|p| rec.piece_verified.get(p as usize).copied().unwrap_or(false))
            .map(|v| NextPreloadView { file_index: v.file_index, active: v.active, ready: v.ready })
    }

    fn idle_record(&self, id: &str) -> Arc<Mutex<IdleRecord>> {
        self.idle_states.lock().entry(id.to_string()).or_default().clone()
    }

    /// Go `beginPlaybackRead`: mark active and wake a resting session before
    /// the read waits for pieces. Never takes the record lock.
    pub(crate) fn begin_playback_read(&self, id: &str) {
        let state = self.idle_record(id);
        let wake = state.lock().policy.begin_read(Instant::now()) == IdleAction::Wake;
        if wake {
            self.spawn_wake_direct_play(id);
        }
    }

    pub(crate) fn end_playback_read(&self, id: &str) {
        self.idle_record(id).lock().policy.end_read(Instant::now());
    }

    /// Go `wakeDirectPlayFromRead`: the wake takes the record lock, so it
    /// runs on its own thread.
    pub(crate) fn spawn_wake_direct_play(&self, id: &str) {
        let Some(server) = self.self_ref.get().and_then(|w| w.upgrade()) else { return; };
        let id = id.to_string();
        std::thread::spawn(move || server.wake_direct_play(&id));
    }

    /// Go `wakeDirectPlay`: restore a resting session's peers and cap.
    fn wake_direct_play(&self, id: &str) {
        let state = self.idle_record(id);
        let peers = {
            let mut st = state.lock();
            if !st.policy.resting() {
                return;
            }
            st.policy.mark_awake();
            std::mem::take(&mut st.rest_peers)
        };
        let Some(rec_arc) = self.record_by_id(id) else { return; };
        let mut rec = rec_arc.write();
        if rec.torrent.closed() {
            return;
        }
        let mut peers = peers;
        peers.extend(rec.reconnect_peers.iter().cloned());
        rec.torrent.add_peers(peers);
        rec.torrent.set_max_established_conns(self.max_peers.load(Ordering::SeqCst).max(1));
        rec.resting = false;
        drop(rec);
        self.notify_change();
    }

    /// Go `restDirectPlay`: close peer connections, keeping dial addresses.
    fn rest_direct_play(&self, rec: &mut TorrentRecord, state: &Mutex<IdleRecord>) {
        let listen_ports = rec.torrent.inner.peer_listen_ports.read().clone();
        let known: Vec<wt_core::PeerInfo> = rec.torrent.peer_conns().into_iter()
            .filter_map(|pc| {
                let addr = if pc.outgoing {
                    pc.remote_addr
                } else {
                    let port = *listen_ports.get(&pc.remote_addr)?;
                    std::net::SocketAddr::new(pc.remote_addr.ip(), port)
                };
                Some(wt_core::PeerInfo {
                    id: None,
                    addr,
                    source: pc.source,
                    supports_encryption: pc.encrypted,
                    pex: Default::default(),
                    trusted: true,
                })
            })
            .take(REST_PEER_LIMIT)
            .collect();
        let mut st = state.lock();
        // A read woke the session after the tick chose to rest.
        if !st.policy.resting() {
            return;
        }
        st.rest_peers = known;
        rec.torrent.set_max_established_conns(0);
        rec.resting = true;
    }

    /// Go `playbackBufferFilling`: an open reader's urgent or readahead window
    /// still misses pieces.
    fn playback_buffer_filling(&self, rec: &TorrentRecord) -> bool {
        let windows: Vec<_> = self.playback_readers.lock().values()
            .filter(|r| r.id == rec.id && !r.is_closed())
            .filter_map(|r| r.window())
            .collect();
        wt_policy::any_playback_buffer_filling(&windows, |p| rec.torrent.piece_complete(p))
    }

    /// Go reader idle timers (15 s), checked once a second.
    pub(crate) fn park_idle_readers(&self) {
        let readers: Vec<_> = self.playback_readers.lock().values().cloned().collect();
        for reader in readers {
            reader.park_if_idle(Duration::from_secs(15));
        }
    }

    /// Go `directPlayIdleTick`, once a second.
    pub(crate) fn direct_play_idle_tick(&self, now: Instant) {
        let minutes = self.idle_drop_minutes.load(Ordering::SeqCst).max(0) as u64;
        let limit = Duration::from_secs(minutes * 60);
        let records: Vec<_> = self.records.read().values().cloned().collect();
        let mut changed = false;
        for rec_arc in records {
            let mut rec = rec_arc.write();
            let eligible = rec.direct_play && rec.configured && !rec.paused
                && !rec.torrent.closed() && rec.error.is_none();
            let input = IdleTickInput {
                eligible,
                idle_limit: Some(limit),
                open_readers: self.open_reader_count(&rec.id),
                buffer_filling: eligible && self.playback_buffer_filling(&rec),
            };
            let state = self.idle_record(&rec.id);
            let action = state.lock().policy.tick(now, &input);
            match action {
                IdleAction::Rest => self.rest_direct_play(&mut rec, &state),
                IdleAction::Wake => {
                    // The policy already cleared resting; restore peers here.
                    let peers = std::mem::take(&mut state.lock().rest_peers);
                    if !rec.torrent.closed() {
                        rec.torrent.add_peers(peers);
                        rec.torrent.set_max_established_conns(self.max_peers.load(Ordering::SeqCst).max(1));
                    }
                    rec.resting = false;
                }
                IdleAction::None => {}
            }
            let drop_ready = state.lock().policy.drop_ready();
            if rec.idle_drop_ready != drop_ready || action != IdleAction::None {
                rec.idle_drop_ready = drop_ready;
                changed = true;
            }
            // Go refreshes hints as the startup cache saves pieces.
            if rec.direct_play {
                self.refresh_startup_hints(&mut rec);
            }
        }
        if changed {
            self.notify_change();
        }
    }

    /// Go /play: reconnect a resting session while the player launches.
    pub(crate) fn touch_direct_play(&self, id: &str) {
        let state = self.idle_record(id);
        let resting = {
            let mut st = state.lock();
            st.policy.touch(Instant::now());
            st.policy.resting()
        };
        if resting {
            self.spawn_wake_direct_play(id);
        }
    }

    pub(crate) fn forget_idle_state(&self, id: &str) {
        self.idle_states.lock().remove(id);
    }
}

pub(crate) type IdleStates = Mutex<HashMap<String, Arc<Mutex<IdleRecord>>>>;

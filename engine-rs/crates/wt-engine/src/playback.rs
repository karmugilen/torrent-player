//! Playback sessions, priority coordination, and reader handles.
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wt_policy::{
    raise_span, span_from_file, AdaptiveBuffer, FileLayout, IdleDirectPlay, NextPreload, PieceSpan,
    PlaybackWindow, ResumeRange, SeekLatencyStats, ADAPTIVE_BUFFER_MIN_BYTES, DIRECT_STARTUP_HEAD,
    DIRECT_STARTUP_TAIL, PLAYBACK_BOOST_HEAD_BYTES, PLAYBACK_BOOST_READAHEAD_BYTES,
    PLAYBACK_BOOST_TAIL_BYTES, VIDEO_STARTUP_HEAD_BYTES, VIDEO_STARTUP_TAIL_BYTES,
};
use wt_types::PiecePriority;

use crate::stream_http::{self, StreamReader, StreamSource};

// Re-export policy constants under the historical names used by server/tests.
pub use wt_policy::{
    PLAYBACK_READAHEAD_BYTES, PLAYBACK_SCRUB_DEBOUNCE, PLAYBACK_SCRUB_JUMP_BYTES,
    PLAYBACK_URGENT_BYTES,
};

/// Settled forward span = urgent tip + readahead (Go `playbackWindowBytes`).
pub const PLAYBACK_WINDOW_BYTES: i64 = PLAYBACK_URGENT_BYTES + PLAYBACK_READAHEAD_BYTES;

/// Direct Play caps simultaneous readers (Go: 4).
pub const DIRECT_PLAY_MAX_READERS: usize = 4;

/// Callback for `set_readahead_fn` (file pos, contiguous start → bytes).
pub type ReadaheadCallback = Arc<dyn Fn(i64, i64) -> i64 + Send + Sync>;

/// Seekable torrent-file reader backed by wt-core or a test fake.
pub trait ReaderBackend: Send {
    fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize>;
    fn set_readahead(&mut self, bytes: i64);
    fn set_readahead_fn(&mut self, f: Option<ReadaheadCallback>);
    fn set_responsive(&mut self, on: bool);
    fn cancel(&mut self);
    fn close(&mut self);
    fn set_cancel(&mut self, _cancel: wt_core::CancelToken) {}
}

/// One file in a torrent, as needed to open playback.
#[derive(Clone, Debug)]
pub struct PlaybackFileInfo {
    pub index: usize,
    pub name: String,
    pub length: i64,
    pub torrent_offset: i64,
    pub begin_piece: u32,
    pub end_piece: u32,
    pub verified: bool,
}

impl PlaybackFileInfo {
    pub fn layout(&self) -> FileLayout {
        FileLayout {
            torrent_offset: self.torrent_offset,
            length: self.length,
            begin_piece: self.begin_piece,
            end_piece: self.end_piece,
        }
    }
}

/// Snapshot of torrent state for opening a playback session.
#[derive(Clone, Debug)]
pub struct PlaybackTorrentSnapshot {
    pub configured: bool,
    pub direct_play: bool,
    pub file_only: bool,
    pub selected: Vec<usize>,
    pub focused_file: Option<usize>,
    pub files: Vec<PlaybackFileInfo>,
    pub piece_length: i64,
    pub open_reader_count: usize,
}

/// Implemented by `EngineServer`. Isolates playback from server locks.
pub trait PlaybackHost: Send + Sync {
    fn torrent_for_playback(&self, id: &str) -> Option<PlaybackTorrentSnapshot>;
    fn is_shutting_down(&self) -> bool;
    fn payload_allowed(&self) -> bool;
    fn playback_allowed(&self, _id: &str, _file_index: usize) -> bool {
        self.payload_allowed()
    }
    fn ghost_pro(&self) -> bool;
    fn open_file_reader(&self, id: &str, file_index: usize) -> io::Result<Box<dyn ReaderBackend>>;
    fn apply_piece_priorities(&self, id: &str, demand: &HashMap<u32, PiecePriority>);
    fn clear_piece_priorities(&self, id: &str, pieces: &[u32]);
    fn set_request_focus(&self, id: &str, torrent_offset: i64);
    fn set_playback_interest(&self, id: &str, begin: u32, end: u32);
    fn refocus_requests(&self, id: &str);
    fn nudge_urgent(&self, id: &str) -> usize;
    fn on_focus_changed(&self, id: &str, file_index: usize);
    fn piece_complete(&self, id: &str, piece: u32) -> bool;
    fn playback_cache_budget(&self, _id: &str) -> Option<i64> {
        None
    }
    fn refresh_playback_completion(&self, _id: &str, _pieces: &[u32]) {}
    /// Verified torrent bytes, `None` while they are not downloaded.
    fn read_verified(&self, _id: &str, _abs: i64, _length: i64) -> Option<Vec<u8>> {
        None
    }
    fn set_playback_storage_demand(&self, _id: &str, _demand: &HashMap<u32, PiecePriority>) {}
    /// Publish one merged demand to storage and to the scheduler.
    fn publish_demand(&self, id: &str, demand: &HashMap<u32, PiecePriority>) {
        self.set_playback_storage_demand(id, demand);
        self.apply_piece_priorities(id, demand);
    }
    fn playback_readers_changed(&self, _id: &str) {}
    fn on_playback_closed(&self, _id: &str, _file_index: usize) {}
    /// Go `seekStats.recordSeek`: seek-to-first-byte latency.
    fn record_seek(&self, _latency: Duration) {}
    /// Go `beginPlaybackRead` / its returned end func (idle Direct Play).
    fn begin_playback_read(&self, _id: &str) {}
    fn end_playback_read(&self, _id: &str) {}
    /// Go `maybeStartNextPreloadLocked`: playback reached the warm-up point.
    fn playback_near_end(&self, _id: &str, _file_index: usize) {}
}

// -------------------------------------------------------------------------------------------------
// In-memory backend (unit tests + completed-fixture without peers)
// -------------------------------------------------------------------------------------------------

pub struct FakeReaderBackend {
    data: Arc<Vec<u8>>,
    readahead: i64,
    responsive: bool,
    closed: bool,
    cancelled: bool,
    #[allow(dead_code)]
    readahead_fn: Option<ReadaheadCallback>,
}

impl FakeReaderBackend {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self {
            data: Arc::new(data.into()),
            readahead: PLAYBACK_URGENT_BYTES,
            responsive: false,
            closed: false,
            cancelled: false,
            readahead_fn: None,
        }
    }
}

impl ReaderBackend for FakeReaderBackend {
    fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        if self.closed || self.cancelled {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "playback reader closed",
            ));
        }
        if offset < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "negative offset",
            ));
        }
        let off = offset as usize;
        if off >= self.data.len() || buf.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(self.data.len() - off);
        buf[..n].copy_from_slice(&self.data[off..off + n]);
        Ok(n)
    }
    fn set_readahead(&mut self, bytes: i64) {
        self.readahead = bytes.max(0);
    }
    fn set_readahead_fn(&mut self, f: Option<ReadaheadCallback>) {
        self.readahead_fn = f;
    }
    fn set_responsive(&mut self, on: bool) {
        self.responsive = on;
    }
    fn cancel(&mut self) {
        self.cancelled = true;
    }
    fn close(&mut self) {
        self.closed = true;
        self.cancelled = true;
    }
}

// -------------------------------------------------------------------------------------------------
// Priority coordinator (torrent-global max-demand across readers)
// -------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct WindowState {
    #[allow(dead_code)]
    window: PlaybackWindow,
}

pub struct PlaybackPriorityCoordinator {
    inner: Mutex<CoordinatorInner>,
}

/// What a file's index tells playback, once it has been read.
#[derive(Default)]
struct SeekPlan {
    /// Keyframe pieces, in fetch order.
    pieces: Vec<u32>,
    /// Keyframe offsets in the file, sorted.
    offsets: Vec<i64>,
    /// Average video rate, from the container's duration.
    bytes_per_sec: Option<f64>,
}

type SeekPieces = Arc<std::sync::OnceLock<SeekPlan>>;

/// Video the settled window holds, so it means the same at any bitrate.
const WINDOW_SECONDS: f64 = 30.0;
const URGENT_SECONDS: f64 = 2.0;
/// Played video kept in RAM for jumping back.
const BEHIND_SECONDS: f64 = 20.0;
/// Bytes after a seek that stand for "playing again": about what a player
/// needs to reach a picture, unlike the first byte.
const SEEK_SPRINT_BYTES: usize = 2 << 20;
/// Keyframe pieces fetched at a time, so priorities can follow the plan's order.
const KEYFRAME_BATCH: usize = 32;

/// Keyframe prefetch budget when pieces stay on disk anyway.
const SEEK_FETCH_DISK_BYTES: i64 = 256 << 20;
/// How long to wait for a video's index to finish downloading.
const SEEK_INDEX_ATTEMPTS: u32 = 120;

struct CoordinatorInner {
    owners: HashMap<u64, HashMap<u32, PiecePriority>>,
    windows: HashMap<u64, WindowState>,
    hints: HashMap<u32, PiecePriority>,
    focus: HashMap<u32, PiecePriority>,
    preview_demand: HashMap<u32, PiecePriority>,
    resume_demand: HashMap<u32, PiecePriority>,
    effective: HashMap<u32, PiecePriority>,
    play_boost: HashMap<usize, bool>,
    adaptive_buffers: HashMap<usize, bool>,
    burst_buffers: HashMap<usize, bool>,
    /// file_index → active session id
    active: HashMap<usize, u64>,
    playing: Option<u64>,
    resume_positions: HashMap<usize, i64>,
    resume_generation: u64,
    /// file_index → pieces at its keyframes, filled once its index is read.
    seek_pieces: HashMap<usize, SeekPieces>,
    closed: bool,
}

impl Default for PlaybackPriorityCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackPriorityCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CoordinatorInner {
                owners: HashMap::new(),
                windows: HashMap::new(),
                hints: HashMap::new(),
                focus: HashMap::new(),
                preview_demand: HashMap::new(),
                resume_demand: HashMap::new(),
                effective: HashMap::new(),
                play_boost: HashMap::new(),
                adaptive_buffers: HashMap::new(),
                burst_buffers: HashMap::new(),
                active: HashMap::new(),
                playing: None,
                resume_positions: HashMap::new(),
                resume_generation: 0,
                seek_pieces: HashMap::new(),
                closed: false,
            }),
        }
    }

    pub fn set_play_boost(&self, file_index: usize, boost: bool) {
        let mut g = self.inner.lock();
        if boost {
            g.play_boost.insert(file_index, true);
        } else {
            g.play_boost.remove(&file_index);
        }
    }

    pub fn play_boost(&self, file_index: usize) -> bool {
        self.inner
            .lock()
            .play_boost
            .get(&file_index)
            .copied()
            .unwrap_or(false)
    }

    pub fn set_buffer_experiments(&self, file_index: usize, adaptive: bool, burst: bool) {
        let mut g = self.inner.lock();
        if adaptive {
            g.adaptive_buffers.insert(file_index, true);
        } else {
            g.adaptive_buffers.remove(&file_index);
        }
        if burst {
            g.burst_buffers.insert(file_index, true);
        } else {
            g.burst_buffers.remove(&file_index);
        }
    }

    pub fn adaptive_for(&self, file_index: usize) -> (bool, bool) {
        let g = self.inner.lock();
        (
            g.adaptive_buffers.contains_key(&file_index),
            g.burst_buffers.contains_key(&file_index),
        )
    }

    /// Go `setFocus`: background demand for the focused file.
    pub fn set_focus(&self, demand: HashMap<u32, PiecePriority>) {
        let mut inner = self.inner.lock();
        if inner.focus == demand {
            return;
        }
        inner.focus = demand;
        Self::apply_locked(&mut inner);
    }

    pub fn hints(&self) -> HashMap<u32, PiecePriority> {
        self.inner.lock().hints.clone()
    }

    pub fn set_hints(&self, demand: HashMap<u32, PiecePriority>) {
        let mut inner = self.inner.lock();
        inner.hints = demand;
        Self::apply_locked(&mut inner);
    }

    pub fn set_preview_demand(&self, demand: HashMap<u32, PiecePriority>) {
        let mut inner = self.inner.lock();
        inner.preview_demand = demand;
        Self::apply_locked(&mut inner);
    }


    pub fn remember_resume(&self, file_index: usize, offset: i64) {
        self.inner.lock().resume_positions.entry(file_index).or_insert(offset);
    }

    pub fn prepare_resume(&self, file_index: usize, offset: i64) {
        let mut inner = self.inner.lock();
        inner.resume_positions.insert(file_index, offset);
        inner.resume_generation += 1;
        inner.resume_demand.clear();
        Self::apply_locked(&mut inner);
    }

    pub fn hold_resume(&self, demand: HashMap<u32, PiecePriority>) -> u64 {
        let mut inner = self.inner.lock();
        inner.resume_generation += 1;
        inner.resume_demand = demand;
        Self::apply_locked(&mut inner);
        inner.resume_generation
    }

    pub fn has_resume_demand(&self) -> bool {
        !self.inner.lock().resume_demand.is_empty()
    }

    pub fn expire_resume(&self, generation: u64) -> Option<HashMap<u32, PiecePriority>> {
        let mut inner = self.inner.lock();
        if inner.resume_generation != generation { return None }
        inner.resume_demand.clear();
        Self::apply_locked(&mut inner);
        Some(inner.effective.clone())
    }

    pub fn take_active(&self, session_id: u64, file_index: usize) -> (Vec<u64>, bool) {
        let mut g = self.inner.lock();
        if g.closed {
            return (Vec::new(), false);
        }
        if g.active.get(&file_index) == Some(&session_id) {
            return (Vec::new(), false);
        }
        g.active.insert(file_index, session_id);
        let others: Vec<u64> = g
            .owners
            .keys()
            .copied()
            .filter(|id| *id != session_id)
            .collect();
        (others, true)
    }

    pub fn may_fetch(&self, session_id: u64, file_index: usize) -> bool {
        let g = self.inner.lock();
        match g.active.get(&file_index) {
            None => true,
            Some(id) => *id == session_id,
        }
    }

    pub fn update(
        &self,
        session_id: u64,
        demand: HashMap<u32, PiecePriority>,
        window: PlaybackWindow,
    ) -> HashMap<u32, PiecePriority> {
        let mut g = self.inner.lock();
        g.owners.insert(session_id, demand);
        g.windows.insert(session_id, WindowState { window });
        Self::apply_locked(&mut g)
    }

    pub fn remove(&self, session_id: u64) -> HashMap<u32, PiecePriority> {
        let mut g = self.inner.lock();
        g.owners.remove(&session_id);
        g.windows.remove(&session_id);
        g.active.retain(|_, id| *id != session_id);
        if g.playing == Some(session_id) {
            g.playing = None;
        }
        Self::apply_locked(&mut g)
    }

    pub fn is_playing(&self, session_id: u64) -> bool {
        self.inner.lock().playing == Some(session_id)
    }

    pub fn set_playing(&self, session_id: u64) {
        self.inner.lock().playing = Some(session_id);
    }

    pub fn preview_position(&self, file_index: usize) -> i64 {
        self.inner.lock().resume_positions.get(&file_index).copied().unwrap_or(0)
    }

    pub fn effective(&self) -> HashMap<u32, PiecePriority> {
        self.inner.lock().effective.clone()
    }

    fn apply_locked(g: &mut CoordinatorInner) -> HashMap<u32, PiecePriority> {
        let mut next: HashMap<u32, PiecePriority> = HashMap::new();
        let raise = |dst: &mut HashMap<u32, PiecePriority>, src: &HashMap<u32, PiecePriority>| {
            for (&i, &p) in src {
                let e = dst.entry(i).or_insert(PiecePriority::None);
                if p > *e {
                    *e = p;
                }
            }
        };
        for d in g.owners.values() {
            raise(&mut next, d);
        }
        raise(&mut next, &g.hints);
        raise(&mut next, &g.focus);
        raise(&mut next, &g.resume_demand);
        raise(&mut next, &g.preview_demand);

        // Cap priorities when multiple readers compete (Go applyLocked).
        let multi = g.owners.len() > 1;
        let preview_first = !g.preview_demand.is_empty() && !g.owners.is_empty();
        for p in next.values_mut() {
            if multi && *p > PiecePriority::High {
                *p = PiecePriority::High;
            }
            if preview_first && *p > PiecePriority::Next {
                *p = PiecePriority::Next;
            }
        }

        g.effective = next.clone();
        next
    }
}

// -------------------------------------------------------------------------------------------------
// Playback session / reader handle
// -------------------------------------------------------------------------------------------------

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub struct PlaybackReader {
    pub id: String,
    pub file_index: usize,
    pub length: i64,
    pub current_offset: AtomicI64,
    pub boost: bool,
    pub adaptive_buffer: bool,
    pub burst_aware_buffer: bool,
    pub next_file_index: Option<usize>,
    display_name: String,
    closed: AtomicBool,
    session: Mutex<Option<PlaybackSession>>,
    read_cancel: Mutex<Option<wt_core::CancelToken>>,
    read_generation: AtomicU64,
    published_window: Mutex<Option<PlaybackWindow>>,
    /// Go `inFlight`: reads in progress are never parked.
    in_flight: AtomicUsize,
    /// End of the latest read (Go re-arms its idle timer per read).
    last_activity: Mutex<Instant>,
    parked: AtomicBool,
}

impl PlaybackReader {
    pub fn from_session(session: PlaybackSession) -> Arc<Self> {
        Arc::new(Self {
            id: session.torrent_id.clone(),
            file_index: session.file_index,
            length: session.length,
            current_offset: AtomicI64::new(session.last_offset),
            boost: session.boost,
            adaptive_buffer: session.adaptive.is_some(),
            burst_aware_buffer: session
                .adaptive
                .as_ref()
                .map(|a| a.burst_aware())
                .unwrap_or(false),
            next_file_index: None,
            display_name: session.file_name.clone(),
            closed: AtomicBool::new(false),
            published_window: Mutex::new(Some(session.window)),
            in_flight: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
            parked: AtomicBool::new(false),
            session: Mutex::new(Some(session)),
            read_cancel: Mutex::new(None),
            read_generation: AtomicU64::new(0),
        })
    }

    /// Open through [`PlaybackHost`] and return the frozen EngineServer shape
    /// `(handle, Arc<PlaybackReader>)`. Caller allocates `handle` (JNI/server
    /// `next_reader_id`) and inserts the Arc into its handle table.
    pub fn open_for_host(
        host: Arc<dyn PlaybackHost>,
        coordinator: Arc<PlaybackPriorityCoordinator>,
        id: &str,
        file_index: usize,
        start: i64,
        handle: i64,
    ) -> io::Result<(i64, Arc<Self>)> {
        let sess = PlaybackSession::open(host, coordinator, id, file_index, start)?;
        Ok((handle, Self::from_session(sess)))
    }

    pub fn read_at(&self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        self.read(buf, offset, false)
    }

    pub fn read_some(&self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        self.read(buf, offset, true)
    }

    fn read(&self, buf: &mut [u8], offset: i64, partial: bool) -> io::Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "playback reader closed",
            ));
        }
        if offset < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid playback offset",
            ));
        }
        if offset >= self.length || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.length - offset) as usize);
        // Cancellation precedes the I/O lock, as in Go readIO/waitCancel. A
        // newer seek can unwind the old blocking read without racing its cursor.
        let (generation, cancel) = {
            let mut active = self.read_cancel.lock();
            if let Some(old) = active.take() {
                old.cancel();
            }
            let cancel = wt_core::CancelToken::default();
            let generation = self.read_generation.fetch_add(1, Ordering::SeqCst) + 1;
            *active = Some(cancel.clone());
            (generation, cancel)
        };
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        // A parked descriptor resumes on its next read (its window republishes).
        self.parked.store(false, Ordering::SeqCst);
        let result = self.read_session(buf, offset, partial, want, generation, cancel);
        *self.last_activity.lock() = Instant::now();
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn read_session(
        &self,
        buf: &mut [u8],
        offset: i64,
        partial: bool,
        want: usize,
        generation: u64,
        cancel: wt_core::CancelToken,
    ) -> io::Result<usize> {
        let mut session = self.session.lock();
        if self.closed.load(Ordering::SeqCst)
            || generation != self.read_generation.load(Ordering::SeqCst)
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "playback closed or superseded",
            ));
        }
        let result = if let Some(sess) = session.as_mut() {
            sess.backend.set_cancel(cancel);
            if sess.cursor != offset {
                sess.focus_pending = true;
            }
            sess.update_window(offset);
            *self.published_window.lock() = Some(sess.window);
            let result = sess.read(&mut buf[..want], offset, partial);
            *self.published_window.lock() = Some(sess.window);
            result
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "playback reader closed",
            ))
        };
        if let Ok(n) = result {
            self.current_offset
                .store(offset + n as i64, Ordering::SeqCst);
        }
        result
    }

    /// Last published playback window (Go `playbackWindows`).
    pub fn window(&self) -> Option<PlaybackWindow> {
        *self.published_window.lock()
    }

    pub fn apply_offset(&self, offset: i64) {
        if let Some(session) = self.session.lock().as_mut() {
            session.focus_pending = true;
            session.update_window(offset);
            *self.published_window.lock() = Some(session.window);
        }
    }

    pub fn finish_focus(&self) {
        if let Some(session) = self.session.lock().as_mut() {
            session.focus_pending = false;
            session.update_window(session.last_offset);
            *self.published_window.lock() = Some(session.window);
        }
    }

    pub fn refresh_window(&self) {
        if self.is_closed() { return; }
        if let Some(mut guard) = self.session.try_lock() {
            if let Some(session) = guard.as_mut() {
                session.update_window(session.last_offset);
                *self.published_window.lock() = Some(session.window);
            }
        }
    }

    pub fn set_play_boost(&self, boost: bool) {
        if let Some(session) = self.session.lock().as_mut() {
            session.boost = boost;
            session
                .coordinator
                .set_play_boost(session.file_index, boost);
            session.update_window(session.last_offset);
            *self.published_window.lock() = Some(session.window);
        }
    }

    /// Go `armIdleLocked`/`idleExpired`: an adaptive-buffer reader that is not
    /// the playing one and has not read for `idle_after` drops its demand, so
    /// an old probe descriptor stops competing with the playhead.
    pub fn park_if_idle(&self, idle_after: Duration) {
        if !self.adaptive_buffer
            || self.closed.load(Ordering::SeqCst)
            || self.parked.load(Ordering::SeqCst)
            || self.in_flight.load(Ordering::SeqCst) != 0
            || self.last_activity.lock().elapsed() < idle_after
        {
            return;
        }
        let playing = self.session.try_lock().is_some_and(|guard| guard.as_ref()
            .is_some_and(|s| s.coordinator.is_playing(s.session_id)));
        if playing {
            // Go re-arms the timer for the playing reader.
            *self.last_activity.lock() = Instant::now();
            return;
        }
        if self.in_flight.load(Ordering::SeqCst) == 0 {
            self.park_idle();
            self.parked.store(true, Ordering::SeqCst);
        }
    }

    pub fn park_idle(&self) {
        if let Some(mut guard) = self.session.try_lock() {
            if let Some(session) = guard.as_mut() {
                session.backend.set_readahead(0);
                let effective = session.coordinator.remove(session.session_id);
                if let Some(host) = &session.host {
                    host.publish_demand(&session.torrent_id, &effective);
                }
                session.window = PlaybackWindow::default();
                *self.published_window.lock() = Some(session.window);
            }
        }
    }

    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(cancel) = self.read_cancel.lock().take() {
            cancel.cancel();
        }
        self.read_generation.fetch_add(1, Ordering::SeqCst);
        let host = {
            let mut guard = self.session.lock();
            guard.take().and_then(|mut sess| {
                let host = sess.host.clone();
                sess.close();
                host
            })
        };
        if let Some(host) = host {
            host.on_playback_closed(&self.id, self.file_index);
            host.playback_readers_changed(&self.id);
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn cancel_pending_read(&self) {
        if let Some(cancel) = self.read_cancel.lock().as_ref() {
            cancel.cancel();
        }
    }

    pub fn window_snapshot(&self) -> Option<PlaybackWindow> {
        if self.closed.load(Ordering::SeqCst) {
            return None;
        }
        if let Some(session) = self.session.try_lock() {
            return session.as_ref().map(|s| s.window);
        }
        *self.published_window.lock()
    }
}

impl StreamReader for PlaybackReader {
    fn length(&self) -> i64 {
        self.length
    }
    fn display_name(&self) -> &str {
        &self.display_name
    }
    fn read_at(&self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        PlaybackReader::read_at(self, buf, offset)
    }
    fn read_some(&self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        PlaybackReader::read_some(self, buf, offset)
    }
    fn close(&self) {
        PlaybackReader::close(self);
    }
    fn is_closed(&self) -> bool {
        PlaybackReader::is_closed(self)
    }
}

pub struct PlaybackSession {
    pub session_id: u64,
    pub torrent_id: String,
    pub file_index: usize,
    pub file_name: String,
    pub length: i64,
    pub torrent_offset: i64,
    pub piece_length: i64,
    pub begin_piece: u32,
    pub end_piece: u32,
    pub boost: bool,
    pub direct_play: bool,
    pub open_phase: bool,
    pub scrubbing: bool,
    pub head_boost_active: bool,
    pub tail_boost_active: bool,
    pub have_offset: bool,
    pub last_offset: i64,
    pub last_scrub: Option<Instant>,
    pub forward_bytes: i64,
    pub startup_head_bytes: i64,
    pub startup_tail_bytes: i64,
    pub head_span: PieceSpan,
    pub tail_span: PieceSpan,
    pub window: PlaybackWindow,
    pub adaptive: Option<AdaptiveBuffer>,
    pub resume: ResumeRange,
    pub next_preload: NextPreload,
    pub idle: IdleDirectPlay,
    pub seek_stats: SeekLatencyStats,
    pub coordinator: Arc<PlaybackPriorityCoordinator>,
    host: Option<Arc<dyn PlaybackHost>>,
    backend: Box<dyn ReaderBackend>,
    closed: bool,
    focus_pending: bool,
    /// Go `seekStarted`: set by a jump, consumed by the first delivered byte.
    seek_started: Option<Instant>,
    /// Go `preloadAsked`: next-episode warm-up requested for this session.
    preload_asked: bool,
    cursor: i64,
    head_read: bool,
    tail_read: bool,
    demand_since: Instant,
    demand_bytes: usize,
    demand_rate: f64,
    seek_pieces: Option<SeekPieces>,
    opened_at: Instant,
    first_byte_logged: bool,
    /// Since the last seek: when it began, and bytes delivered so far.
    seek_sprint: Option<(Instant, usize)>,
    /// When this read began and the previous one returned, for seek timing.
    read_began: Instant,
    last_read_end: Instant,
    _fast_core: wt_wire::fast_core::PlaybackHold,
    /// Window and keyframe count last published, and when.
    last_applied: Option<(PlaybackWindow, usize, Instant)>,
}

impl PlaybackSession {
    /// Open a session through [`PlaybackHost`]. `start < 0` uses resume offset 0
    /// until the host wires ResumeRange persistence.
    pub fn open(
        host: Arc<dyn PlaybackHost>,
        coordinator: Arc<PlaybackPriorityCoordinator>,
        id: &str,
        file_index: usize,
        start: i64,
    ) -> io::Result<Self> {
        if host.is_shutting_down() {
            return Err(io::Error::other("torrent is closing"));
        }
        let snap = host
            .torrent_for_playback(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "torrent not found"))?;
        if !snap.configured {
            return Err(io::Error::other("file is not configured for playback"));
        }
        if file_index >= snap.files.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid file index",
            ));
        }
        let file = &snap.files[file_index];
        if !host.playback_allowed(id, file_index) && !file.verified {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "payload transfers are paused"));
        }
        let selected = snap.selected.contains(&file_index);
        if !selected && (snap.direct_play || !file.verified) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file is not configured for playback",
            ));
        }
        if snap.direct_play && snap.open_reader_count >= DIRECT_PLAY_MAX_READERS {
            return Err(io::Error::other("too many simultaneous playback readers"));
        }

        let resumed_at = coordinator.preview_position(file_index);
        let boost = coordinator.play_boost(file_index);
        let (adaptive_on, burst_on) = coordinator.adaptive_for(file_index);
        let mut forward = PLAYBACK_WINDOW_BYTES;
        let (mut head_bytes, mut tail_bytes) = (VIDEO_STARTUP_HEAD_BYTES, VIDEO_STARTUP_TAIL_BYTES);
        if boost {
            forward = PLAYBACK_URGENT_BYTES + PLAYBACK_BOOST_READAHEAD_BYTES;
            head_bytes = PLAYBACK_BOOST_HEAD_BYTES;
            tail_bytes = PLAYBACK_BOOST_TAIL_BYTES;
        }
        // Stream-mode / completion refresh uses the Direct Play durable windows.
        if snap.direct_play && !boost {
            // Keep default forward; durable startup spans use DIRECT_* below when
            // storage marks refresh — host can set boost flags later.
            forward = 4 << 20;
            head_bytes = DIRECT_STARTUP_HEAD;
            tail_bytes = DIRECT_STARTUP_TAIL;
        }

        let mut backend = host.open_file_reader(id, file_index)?;
        backend.set_readahead(PLAYBACK_URGENT_BYTES);
        backend.set_readahead_fn(None);

        let layout = file.layout();
        let head_span = span_from_file(&layout, snap.piece_length, 0, head_bytes.min(file.length));
        let tail_span = if file.length > tail_bytes {
            span_from_file(
                &layout,
                snap.piece_length,
                file.length - tail_bytes,
                file.length,
            )
        } else {
            PieceSpan::default()
        };

        host.set_playback_interest(id, file.begin_piece, file.end_piece);

        let focus_changed = (snap.direct_play || snap.file_only)
            && wt_policy::is_video_file_name(&file.name)
            && snap.focused_file != Some(file_index);
        if focus_changed {
            host.on_focus_changed(id, file_index);
        }

        let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
        let mut sess = Self {
            session_id,
            torrent_id: id.to_string(),
            file_index,
            file_name: file.name.clone(),
            length: file.length,
            torrent_offset: file.torrent_offset,
            piece_length: snap.piece_length,
            begin_piece: file.begin_piece,
            end_piece: file.end_piece,
            boost,
            direct_play: snap.direct_play,
            open_phase: true,
            scrubbing: false,
            head_boost_active: !head_span.empty(),
            tail_boost_active: !tail_span.empty(),
            have_offset: false,
            last_offset: 0,
            last_scrub: None,
            forward_bytes: forward,
            startup_head_bytes: head_bytes,
            startup_tail_bytes: tail_bytes,
            head_span,
            tail_span,
            window: PlaybackWindow::default(),
            adaptive: if adaptive_on || burst_on {
                let mut a = AdaptiveBuffer::new(burst_on);
                let _ = a.observe(0.0, forward.max(ADAPTIVE_BUFFER_MIN_BYTES));
                Some(a)
            } else {
                None
            },
            resume: ResumeRange::default(),
            next_preload: NextPreload::default(),
            idle: IdleDirectPlay::default(),
            seek_stats: SeekLatencyStats::default(),
            coordinator,
            host: Some(host),
            backend,
            closed: false,
            focus_pending: true,
            seek_started: None,
            preload_asked: false,
            cursor: -1,
            head_read: false,
            tail_read: false,
            demand_since: Instant::now(),
            demand_bytes: 0,
            demand_rate: 0.0,
            seek_pieces: None,
            opened_at: Instant::now(),
            first_byte_logged: false,
            seek_sprint: None,
            read_began: Instant::now(),
            last_read_end: Instant::now(),
            _fast_core: wt_wire::fast_core::PlaybackHold::new(),
            last_applied: None,
        };

        let resume_offset = if start >= 0 && start < sess.length {
            start
        } else {
            0
        };
        // A seek arrives as a new request, so this open is where it is detected:
        // there is no earlier offset for update_window to compare with.
        if !(0..PLAYBACK_SCRUB_JUMP_BYTES).contains(&(resume_offset - resumed_at))
            && resume_offset >= head_bytes
            && resume_offset < sess.length.saturating_sub(tail_bytes)
        {
            let now = Instant::now();
            sess.seek_started = Some(now);
            wt_types::events::seek_trace::begin();
            sess.scrubbing = true;
            sess.last_scrub = Some(now);
        }
        sess.update_window(resume_offset);
        Ok(sess)
    }

    /// Build a session over an injected backend (unit tests).
    pub fn open_with_backend(
        coordinator: Arc<PlaybackPriorityCoordinator>,
        torrent_id: &str,
        file: PlaybackFileInfo,
        piece_length: i64,
        backend: Box<dyn ReaderBackend>,
        start: i64,
        boost: bool,
    ) -> Self {
        let layout = file.layout();
        let head_bytes = if boost {
            PLAYBACK_BOOST_HEAD_BYTES
        } else {
            VIDEO_STARTUP_HEAD_BYTES
        };
        let tail_bytes = if boost {
            PLAYBACK_BOOST_TAIL_BYTES
        } else {
            VIDEO_STARTUP_TAIL_BYTES
        };
        let head_span = span_from_file(&layout, piece_length, 0, head_bytes.min(file.length));
        let tail_span = if file.length > tail_bytes {
            span_from_file(&layout, piece_length, file.length - tail_bytes, file.length)
        } else {
            PieceSpan::default()
        };
        let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
        let mut sess = Self {
            session_id,
            torrent_id: torrent_id.into(),
            file_index: file.index,
            file_name: file.name,
            length: file.length,
            torrent_offset: file.torrent_offset,
            piece_length,
            begin_piece: file.begin_piece,
            end_piece: file.end_piece,
            boost,
            direct_play: false,
            open_phase: true,
            scrubbing: false,
            head_boost_active: !head_span.empty(),
            tail_boost_active: !tail_span.empty(),
            have_offset: false,
            last_offset: 0,
            last_scrub: None,
            forward_bytes: if boost {
                PLAYBACK_URGENT_BYTES + PLAYBACK_BOOST_READAHEAD_BYTES
            } else {
                PLAYBACK_WINDOW_BYTES
            },
            startup_head_bytes: head_bytes,
            startup_tail_bytes: tail_bytes,
            head_span,
            tail_span,
            window: PlaybackWindow::default(),
            adaptive: None,
            resume: ResumeRange::default(),
            next_preload: NextPreload::default(),
            idle: IdleDirectPlay::default(),
            seek_stats: SeekLatencyStats::default(),
            coordinator,
            host: None,
            backend,
            closed: false,
            focus_pending: true,
            seek_started: None,
            preload_asked: false,
            cursor: -1,
            head_read: false,
            tail_read: false,
            demand_since: Instant::now(),
            demand_bytes: 0,
            demand_rate: 0.0,
            seek_pieces: None,
            opened_at: Instant::now(),
            first_byte_logged: false,
            seek_sprint: None,
            read_began: Instant::now(),
            last_read_end: Instant::now(),
            _fast_core: wt_wire::fast_core::PlaybackHold::new(),
            last_applied: None,
        };
        let off = if start >= 0 && start < sess.length {
            start
        } else {
            0
        };
        sess.update_window(off);
        sess
    }

    pub fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        self.read(buf, offset, false)
    }

    pub fn read_some(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        self.read(buf, offset, true)
    }

    /// Go `defer srv.beginPlaybackRead(rec)()`: idle Direct Play sees the
    /// read as activity for its whole duration, including the wait.
    fn read(&mut self, buf: &mut [u8], offset: i64, partial: bool) -> io::Result<usize> {
        self.read_began = Instant::now();
        let host = self.host.clone();
        if let Some(host) = &host {
            host.begin_playback_read(&self.torrent_id);
        }
        let result = self.read_inner(buf, offset, partial);
        if let Some(host) = &host {
            host.end_playback_read(&self.torrent_id);
        }
        self.last_read_end = Instant::now();
        result
    }

    fn read_inner(&mut self, buf: &mut [u8], offset: i64, _partial: bool) -> io::Result<usize> {
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "playback reader closed",
            ));
        }
        if offset < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid offset",
            ));
        }
        if offset >= self.length || buf.is_empty() {
            return Ok(0);
        }
        if self.host.as_ref().is_some_and(|h| !h.playback_allowed(&self.torrent_id, self.file_index)) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "payload transfers are paused"));
        }
        let want = buf.len().min((self.length - offset) as usize);
        if self.cursor != offset {
            self.focus_pending = true;
        }
        self.update_window(offset);
        let focused = self.focus_pending;
        let mut n = 0;
        while n < want {
            if n > 0 && self.direct_play {
                self.update_window(offset + n as i64);
            }
            let got = self.backend.read_at(&mut buf[n..want], offset + n as i64)?;
            if got == 0 {
                break;
            }
            n += got;
            self.finish_focus();
            if _partial {
                break;
            }
        }
        if n > 0 {
            if !self.first_byte_logged {
                self.first_byte_logged = true;
                wt_types::events::record("first_byte", format!("{}ms file={}", self.opened_at.elapsed().as_millis(), self.file_index));
            }
            self.cursor = offset + n as i64;
            if let Some((started, bytes)) = &mut self.seek_sprint {
                wt_types::events::seek_trace::note_read(
                    self.read_began.elapsed().as_micros() as u64,
                    self.read_began.saturating_duration_since(self.last_read_end).as_micros() as u64,
                );
                *bytes += n;
                if *bytes >= SEEK_SPRINT_BYTES {
                    wt_types::events::record("seek_2mib", format!("{}ms file={}", started.elapsed().as_millis(), self.file_index));
                    let stages = wt_types::events::seek_trace::finish(started.elapsed().as_millis());
                    crate::jni::log_info(&format!("seek_stages {stages}"));
                    self.seek_sprint = None;
                }
            }
            self.demand_bytes += n;
            let elapsed = self.demand_since.elapsed().as_secs_f64();
            if elapsed >= 0.5 {
                self.demand_rate = self.demand_bytes as f64 / elapsed;
                self.demand_bytes = 0;
                self.demand_since = Instant::now();
            }
            self.finish_focus();
            // Go finishFocusLocked: a seek's first bytes re-centre the window
            // on the read position with steady urgency. Sequential reads
            // advance it with the bytes delivered.
            self.update_window(if focused { offset } else { self.cursor.saturating_sub(1) });
        }
        if !self.open_phase {
            self.coordinator.set_playing(self.session_id);
            let head = if self.boost { self.startup_head_bytes } else { VIDEO_STARTUP_HEAD_BYTES };
            let tail = if self.boost { self.startup_tail_bytes } else { VIDEO_STARTUP_TAIL_BYTES };
            if n > 0 && offset >= head && offset < self.length - tail {
                self.coordinator.inner.lock().resume_positions.insert(self.file_index, offset);
            }
        }
        Ok(n)
    }

    /// Go `finishFocusLocked`: the pending focus has delivered bytes.
    fn finish_focus(&mut self) {
        self.focus_pending = false;
        if let Some(started) = self.seek_started.take() {
            if let Some(host) = &self.host {
                host.record_seek(started.elapsed());
                wt_types::events::seek_trace::mark(wt_types::events::seek_trace::Stage::Player);
                wt_types::events::record("seek", format!("{}ms file={}", started.elapsed().as_millis(), self.file_index));
                self.seek_sprint = Some((started, 0));
            }
        }
    }

    pub fn update_window(&mut self, mut offset: i64) {
        if self.length <= 0 || self.piece_length <= 0 {
            return;
        }
        if offset < 0 {
            offset = 0;
        }
        if offset >= self.length {
            offset = self.length - 1;
        }
        let mut refocus = self.focus_pending && (!self.have_offset || offset != self.last_offset);

        let now = Instant::now();
        let head_limit = self.startup_head_bytes.min(self.length);
        let has_tail = self.length > self.startup_tail_bytes;
        let tail_limit = if has_tail {
            self.length - self.startup_tail_bytes
        } else {
            0
        };
        let in_head = offset < head_limit;
        let in_tail = has_tail && offset >= tail_limit;
        let in_body = !in_head && !in_tail;

        if self.open_phase {
            if in_body
                || (self.adaptive.is_some()
                    && self.have_offset
                    && offset >= 256 << 10
                    && offset > self.last_offset
                    && offset - self.last_offset < PLAYBACK_SCRUB_JUMP_BYTES)
            {
                self.open_phase = false;
            } else if in_head {
                if self.head_read && self.tail_read {
                    self.open_phase = false;
                } else {
                    self.head_read = true;
                }
            } else if in_tail {
                self.tail_read = true;
            }
        }

        if self.scrubbing {
            if let Some(t) = self.last_scrub {
                if now.duration_since(t) >= PLAYBACK_SCRUB_DEBOUNCE {
                    self.scrubbing = false;
                }
            }
        }

        let was_positioned = self.have_offset;
        if self.have_offset {
            let delta = offset - self.last_offset;
            // A read that starts on a keyframe, not where the last one ended,
            // is a seek however short the jump.
            let on_keyframe = self.focus_pending
                && delta > 0
                && self.seek_pieces.as_ref().and_then(|p| p.get())
                    .is_some_and(|plan| plan.offsets.binary_search(&offset).is_ok());
            if !self.open_phase && (!(0..PLAYBACK_SCRUB_JUMP_BYTES).contains(&delta) || on_keyframe) {
                self.focus_pending = true;
                self.seek_started = Some(now);
                wt_types::events::seek_trace::begin();
                self.scrubbing = true;
                self.last_scrub = Some(now);
                refocus = true;
            } else if self.scrubbing {
                self.last_scrub = Some(now);
            }
        }
        self.last_offset = offset;
        self.have_offset = true;
        // Go maybeStartNextPreloadLocked: opening probes and scrubs never
        // count as reaching the end.
        if !self.preload_asked && !self.open_phase && !self.scrubbing && self.length > 0
            && offset as f64 >= self.length as f64 * wt_policy::NEXT_PRELOAD_AT
        {
            self.preload_asked = true;
            if let Some(host) = &self.host {
                host.playback_near_end(&self.torrent_id, self.file_index);
            }
        }

        let (_others, became) = self
            .coordinator
            .take_active(self.session_id, self.file_index);
        let _ = (was_positioned, became);

        if !self.coordinator.may_fetch(self.session_id, self.file_index) {
            // Parked: clear our demand.
            let demand = self.coordinator.remove(self.session_id);
            if let Some(host) = &self.host {
                host.set_request_focus(&self.torrent_id, self.torrent_offset + offset);
                host.apply_piece_priorities(&self.torrent_id, &demand);
            }
            return;
        }

        let bytes_per_sec = self.seek_pieces.as_ref().and_then(|p| p.get()).and_then(|plan| plan.bytes_per_sec);
        let settled = match bytes_per_sec {
            // Seconds only ever widen the window: narrowing it for low-bitrate
            // video left less ready when a seek landed.
            Some(rate) if !self.boost && !self.direct_play => {
                ((rate * WINDOW_SECONDS) as i64).clamp(self.forward_bytes, 128 << 20)
            }
            _ => self.forward_bytes,
        };
        let mut forward = if let Some(ref mut adaptive) = self.adaptive {
            adaptive
                .observe(self.demand_rate, settled)
                .max(PLAYBACK_URGENT_BYTES)
        } else {
            settled
        };
        let ghost = self.host.as_ref().is_some_and(|h| h.ghost_pro());
        let mut urgent_bytes = bytes_per_sec
            .map_or(PLAYBACK_URGENT_BYTES, |rate| ((rate * URGENT_SECONDS) as i64).clamp(PLAYBACK_URGENT_BYTES, 16 << 20));
        let mut lookahead_bytes = 0;
        if ghost {
            forward = forward.max(64 << 20).max(6 * self.piece_length);
            if self.boost {
                forward = forward.max(96 << 20);
            }
            lookahead_bytes = (32 << 20).max(4 * self.piece_length);
            if !self.open_phase && !self.focus_pending {
                urgent_bytes = urgent_bytes.max(16 << 20).max(2 * self.piece_length);
            }
        }
        forward = forward.max(urgent_bytes);
        let budget = self
            .host
            .as_ref()
            .and_then(|h| h.playback_cache_budget(&self.torrent_id));
        self.start_seek_index(budget);
        let plan = self.seek_pieces.clone();
        let keyframes: &[u32] = plan.as_ref().and_then(|p| p.get()).map_or(&[], |plan| &plan.pieces);
        // Keyframe pieces stay in the RAM cache; the window gets the rest.
        let cache_budget = budget;
        let budget = budget.map(|b| b - keyframes.len() as i64 * self.piece_length);
        let readers = {
            let inner = self.coordinator.inner.lock();
            (inner.owners.len() + usize::from(!inner.owners.contains_key(&self.session_id))).max(1)
                as i64
        };
        let layout = FileLayout {
            torrent_offset: self.torrent_offset,
            length: self.length,
            begin_piece: self.begin_piece,
            end_piece: self.end_piece,
        };
        let mut urgent = span_from_file(
            &layout,
            self.piece_length,
            offset,
            (offset + urgent_bytes).min(self.length),
        );
        let mut readahead = span_from_file(
            &layout,
            self.piece_length,
            (offset + urgent_bytes).min(self.length),
            (offset + forward).min(self.length),
        );
        let mut lookahead = span_from_file(
            &layout,
            self.piece_length,
            (offset + forward).min(self.length),
            (offset + forward + lookahead_bytes).min(self.length),
        );
        let mut reader_ra = forward;
        if let Some(budget) = budget.filter(|&n| n >= self.piece_length) {
            let reserve = (budget / 20).max(self.piece_length);
            let reserve = if reserve >= budget { 0 } else { reserve };
            let allowance = ((budget - reserve) / readers / self.piece_length).max(1) as u32;
            let allowance = allowance.min((budget / self.piece_length / readers).max(1) as u32);
            let (ahead, forward_pieces, urgent_pieces) = if ghost && allowance >= 3 {
                let ahead = (allowance / 4).max(1);
                let forward = allowance - ahead;
                (allowance, forward, (forward / 4).max(1).min(forward - 1))
            } else {
                let forward = ((forward.max(self.piece_length) + self.piece_length - 1)
                    / self.piece_length) as u32;
                let forward = forward.min(allowance).max(1);
                let tip = ((urgent_bytes + self.piece_length - 1) / self.piece_length) as u32;
                (forward, forward, tip.min(forward).max(1))
            };
            let play = ((self.torrent_offset + offset) / self.piece_length) as u32;
            let end = (play + ahead).min(self.end_piece);
            urgent = PieceSpan {
                begin: play,
                end: (play + urgent_pieces).min(end),
            };
            readahead = PieceSpan {
                begin: urgent.end,
                end: (play + forward_pieces).min(end),
            };
            lookahead = PieceSpan {
                begin: readahead.end,
                end,
            };
            reader_ra = forward_pieces as i64 * self.piece_length;
        }
        if self.open_phase {
            urgent = span_from_file(
                &layout,
                self.piece_length,
                offset,
                (offset + PLAYBACK_URGENT_BYTES).min(self.length),
            );
            readahead = PieceSpan::default();
            lookahead = PieceSpan::default();
            reader_ra = PLAYBACK_URGENT_BYTES;
        }
        let window = PlaybackWindow {
            urgent,
            readahead,
            lookahead,
            open_phase: self.open_phase,
            head_boost: self.head_boost_active && self.open_phase,
            tail_boost: self.tail_boost_active && self.open_phase,
        };
        let mut demand = HashMap::new();
        raise_span(&mut demand, urgent, PiecePriority::Next);
        raise_span(&mut demand, readahead, PiecePriority::Next);
        raise_span(&mut demand, lookahead, PiecePriority::Next);
        let tip_bytes = if self.open_phase { PLAYBACK_URGENT_BYTES } else { urgent_bytes.min(4 << 20) };
        let tip_pieces = ((tip_bytes + self.piece_length - 1) / self.piece_length) as u32;
        raise_span(
            &mut demand,
            PieceSpan {
                begin: urgent.begin,
                end: urgent.end.min(urgent.begin + tip_pieces +
                    wt_policy::constants::URGENT_NOW_EXTRA_PIECES.load(Ordering::Relaxed).max(0) as u32),
            },
            PiecePriority::Now,
        );
        if window.head_boost {
            raise_span(&mut demand, self.head_span, PiecePriority::Now);
        }
        if window.tail_boost {
            raise_span(&mut demand, self.tail_span, PiecePriority::Now);
        }
        self.backend.set_readahead(reader_ra);
        // Serve blocks as they arrive, headers included: a player may need a
        // few KiB of a 16 MiB piece. The reader ends the stream if a piece it
        // served fails its hash (CoreReaderBackend), and the player reopens.
        self.backend.set_responsive(true);
        if let Some(host) = &self.host {
            let entering: Vec<u32> = demand
                .keys()
                .copied()
                .filter(|&p| {
                    !self.window.urgent.contains(p)
                        && !self.window.readahead.contains(p)
                        && !self.window.lookahead.contains(p)
                })
                .collect();
            host.refresh_playback_completion(&self.torrent_id, &entering);
        }
        self.window = window;
        // update_window runs several times per read. Republishing an unchanged
        // window takes the piece table's write lock for nothing; the periodic
        // refresh heals priorities that other paths changed.
        let unchanged = !refocus
            && self.last_applied.is_some_and(|(w, count, at)| {
                w == window && count == keyframes.len() && at.elapsed() < Duration::from_secs(1)
            });
        if !unchanged {
            let mut pending = 0;
            for &piece in keyframes {
                if !self.host.as_ref().is_some_and(|h| h.piece_complete(&self.torrent_id, piece)) {
                    pending += 1;
                    if pending > KEYFRAME_BATCH {
                        break;
                    }
                }
                demand.entry(piece).or_insert(PiecePriority::High);
            }
            self.last_applied = Some((window, keyframes.len(), now));
            let effective = self.coordinator.update(self.session_id, demand, window);
            if let Some(host) = &self.host {
                // Just-played pieces are protected from eviction, not fetched.
                let mut protected = effective.clone();
                let behind = bytes_per_sec.map_or(8 << 20, |rate| ((rate * BEHIND_SECONDS) as i64).clamp(4 << 20, 32 << 20));
                let behind = cache_budget.map_or(behind, |b| behind.min(b / 8));
                let play = ((self.torrent_offset + offset) / self.piece_length) as u32;
                let count = (behind / self.piece_length) as u32;
                for piece in play.saturating_sub(count).max(self.begin_piece)..play {
                    protected.entry(piece).or_insert(PiecePriority::Normal);
                }
                host.set_playback_storage_demand(&self.torrent_id, &protected);
                host.apply_piece_priorities(&self.torrent_id, &effective);
            }
        }
        if let Some(host) = &self.host {
            // Focus after priorities, so a woken peer never refills from the
            // old window just before it is cancelled.
            host.set_request_focus(&self.torrent_id, self.torrent_offset + offset);
            if refocus {
                // Publish urgent priorities first, then release old background
                // requests so startup/seek bytes can enter a full pipeline.
                host.refocus_requests(&self.torrent_id);
            }
        }
    }

    /// Once per file, after opening: read the video's index in the background
    /// and work out which pieces hold its keyframes.
    fn start_seek_index(&mut self, ram_budget: Option<i64>) {
        if self.open_phase || self.seek_pieces.is_some() {
            return;
        }
        let Some(host) = self.host.clone() else {
            return;
        };
        let (pieces, started) = {
            let mut inner = self.coordinator.inner.lock();
            match inner.seek_pieces.get(&self.file_index) {
                Some(pieces) => (pieces.clone(), true),
                None => {
                    let pieces = SeekPieces::default();
                    inner.seek_pieces.insert(self.file_index, pieces.clone());
                    (pieces, false)
                }
            }
        };
        self.seek_pieces = Some(pieces.clone());
        if started {
            return;
        }
        let id = self.torrent_id.clone();
        let (file_start, length) = (self.torrent_offset, self.length);
        let (piece_length, begin, end) = (self.piece_length, self.begin_piece, self.end_piece);
        // Direct Play keeps pieces in RAM only: a quarter of it for keyframes.
        let max_bytes = ram_budget.map_or(SEEK_FETCH_DISK_BYTES, |budget| budget / 4);
        let _ = std::thread::Builder::new().name("wt-seek-index".into()).spawn(move || {
            let read = |abs: i64, len: i64| host.read_verified(&id, file_start + abs, len);
            for _ in 0..SEEK_INDEX_ATTEMPTS {
                if host.is_shutting_down() {
                    return;
                }
                if let Some(index) = crate::seek_index::video_index(&read, length) {
                    let mut offsets = index.offsets;
                    offsets.sort_unstable();
                    let _ = pieces.set(SeekPlan {
                        pieces: crate::seek_index::pieces_for_keyframes(
                            &offsets, file_start, piece_length, begin, end, max_bytes,
                        ),
                        offsets,
                        bytes_per_sec: index.duration.filter(|&d| d >= 1.0).map(|d| length as f64 / d),
                    });
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.backend.cancel();
        self.backend.close();
        let effective = self.coordinator.remove(self.session_id);
        if let Some(host) = &self.host {
            host.publish_demand(&self.torrent_id, &effective);
            // The playhead must not outlive playback: it kept a saved download
            // on playback queues long after the video was closed.
            if self.coordinator.inner.lock().owners.is_empty() {
                host.set_request_focus(&self.torrent_id, -1);
            }
        }
    }
}

impl Drop for PlaybackSession {
    fn drop(&mut self) {
        self.close();
    }
}

// -------------------------------------------------------------------------------------------------
// Stream server wiring
// -------------------------------------------------------------------------------------------------

/// `(id, file_index, range_start)`; the start is `-1` when the request has none.
pub type SeekablePlaybackResolver =
    Arc<dyn Fn(&str, usize, i64) -> io::Result<Arc<PlaybackReader>> + Send + Sync>;
struct SeekableResolverSource(SeekablePlaybackResolver);

impl StreamSource for SeekableResolverSource {
    fn open(&self, id: &str, index: usize) -> io::Result<Arc<dyn StreamReader>> {
        self.open_at(id, index, -1)
    }
    fn open_at(&self, id: &str, index: usize, start: i64) -> io::Result<Arc<dyn StreamReader>> {
        (self.0)(id, index, start).map(|reader| reader as Arc<dyn StreamReader>)
    }
}

pub async fn start_stream_server_seekable(
    port: u16,
    resolver: SeekablePlaybackResolver,
) -> io::Result<Arc<StreamServer>> {
    stream_http::start_stream_server(port, Arc::new(SeekableResolverSource(resolver))).await
}


pub use crate::stream_http::StreamServer;

// -------------------------------------------------------------------------------------------------
// Unit tests
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file(len: i64, piece_len: i64) -> PlaybackFileInfo {
        let pieces = ((len + piece_len - 1) / piece_len) as u32;
        PlaybackFileInfo {
            index: 0,
            name: "movie.mp4".into(),
            length: len,
            torrent_offset: 0,
            begin_piece: 0,
            end_piece: pieces,
            verified: true,
        }
    }

    #[test]
    fn open_positions_window_at_start() {
        let coord = Arc::new(PlaybackPriorityCoordinator::new());
        let piece_len = 1 << 20;
        let file = sample_file(64 << 20, piece_len);
        let data = vec![0u8; file.length as usize];
        let mut sess = PlaybackSession::open_with_backend(
            coord.clone(),
            "t1",
            file,
            piece_len,
            Box::new(FakeReaderBackend::new(data)),
            0,
            false,
        );
        assert!(sess.open_phase);
        assert!(!sess.window.urgent.empty());
        let mut buf = [0u8; 16];
        assert_eq!(sess.read_at(&mut buf, 0).unwrap(), 16);
        // Body read past head should eventually leave open phase.
        let body = VIDEO_STARTUP_HEAD_BYTES + 1;
        let _ = sess.read_at(&mut buf, body);
        assert!(!sess.open_phase);
        sess.close();
    }

    #[test]
    fn scrub_jump_marks_scrubbing_and_refocuses_window() {
        let coord = Arc::new(PlaybackPriorityCoordinator::new());
        let piece_len = 256 * 1024;
        let file = sample_file(32 << 20, piece_len);
        let data = vec![7u8; file.length as usize];
        let mut sess = PlaybackSession::open_with_backend(
            coord,
            "t1",
            file,
            piece_len,
            Box::new(FakeReaderBackend::new(data)),
            0,
            false,
        );
        let mut buf = [0u8; 8];
        // Exit open phase via body read.
        let _ = sess.read_at(&mut buf, VIDEO_STARTUP_HEAD_BYTES + 10);
        assert!(!sess.open_phase);
        let before = sess.window.urgent.begin;
        // Big forward jump.
        let _ = sess.read_at(
            &mut buf,
            VIDEO_STARTUP_HEAD_BYTES + PLAYBACK_SCRUB_JUMP_BYTES + 100,
        );
        assert!(sess.scrubbing);
        assert!(sess.window.urgent.begin >= before);
    }

    #[test]
    fn coordinator_caps_now_when_multiple_owners() {
        let coord = PlaybackPriorityCoordinator::new();
        let mut d1 = HashMap::new();
        d1.insert(1, PiecePriority::Now);
        let mut d2 = HashMap::new();
        d2.insert(1, PiecePriority::Now);
        let w = PlaybackWindow::default();
        let _ = coord.update(1, d1, w);
        let eff = coord.update(2, d2, w);
        assert_eq!(eff.get(&1), Some(&PiecePriority::High));
    }

    #[test]
    fn resume_hold_ignores_stale_expiry_and_preserves_newer_generation() {
        let coordinator = PlaybackPriorityCoordinator::new();
        let first = coordinator.hold_resume(HashMap::from([(10, PiecePriority::High)]));
        let second = coordinator.hold_resume(HashMap::from([(14, PiecePriority::High)]));
        assert!(coordinator.expire_resume(first).is_none());
        assert_eq!(coordinator.effective().get(&14), Some(&PiecePriority::High));
        assert!(coordinator.expire_resume(second).unwrap().is_empty());
        assert!(!coordinator.has_resume_demand());
    }

    #[test]
    fn active_reader_parks_others_of_same_file() {
        let coord = Arc::new(PlaybackPriorityCoordinator::new());
        let (others, became) = coord.take_active(10, 0);
        assert!(became);
        assert!(others.is_empty());
        assert!(coord.may_fetch(10, 0));
        let (others, became) = coord.take_active(11, 0);
        assert!(became);
        assert!(!coord.may_fetch(10, 0));
        assert!(coord.may_fetch(11, 0));
        let _ = others;
    }

    #[test]
    fn boost_widens_startup_spans() {
        let coord = Arc::new(PlaybackPriorityCoordinator::new());
        coord.set_play_boost(0, true);
        assert!(coord.play_boost(0));
        let piece_len = 1 << 20;
        let file = sample_file(128 << 20, piece_len);
        let sess = PlaybackSession::open_with_backend(
            coord,
            "t1",
            file,
            piece_len,
            Box::new(FakeReaderBackend::new(vec![0u8; 128 << 20])),
            0,
            true,
        );
        assert!(sess.boost);
        assert!(sess.forward_bytes > PLAYBACK_WINDOW_BYTES);
        assert_eq!(sess.startup_head_bytes, PLAYBACK_BOOST_HEAD_BYTES);
    }
}

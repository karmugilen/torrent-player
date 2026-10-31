//! [`ReaderBackend`] adapter over `wt_core::reader::Reader`.
//!
//! Kept in a sibling module so playback orchestration stays free of wire/conn
//! details. agy-4's `PlaybackHost::open_file_reader` typically returns
//! `Box::new(CoreReaderBackend::from_file(&file))`.

use std::io;
use std::sync::Arc;

use wt_core::reader::Reader;
use wt_core::{File, ReadaheadContext, ReadaheadFn};

use crate::playback::{ReadaheadCallback, ReaderBackend, PLAYBACK_URGENT_BYTES};

/// Go stalled-read refresh points; Claude seek refreshes sooner within the
/// same 45-second budget (`claudeSeekReadAttempts`).
const READ_ATTEMPTS: [u64; 3] = [8, 12, 25];
const SEEK_READ_ATTEMPTS: [u64; 4] = [3, 6, 12, 24];
/// Go `claudeSeekNudgeEvery`.
const SEEK_NUDGE_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// Go `startUrgentNudger`: nudges urgent peers until dropped.
struct UrgentNudger(Arc<std::sync::atomic::AtomicBool>);

impl UrgentNudger {
    fn start(torrent: wt_core::Torrent) -> Option<Self> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("wt-seek-nudge".into())
            .spawn(move || loop {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                torrent.nudge_urgent_peers();
                std::thread::sleep(SEEK_NUDGE_EVERY);
            })
            .ok()?;
        Some(Self(stop))
    }
}

impl Drop for UrgentNudger {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Blocking wt-core reader with readahead / responsive / cancel hooks.
pub struct CoreReaderBackend {
    inner: Reader,
    stream: Option<(Arc<wt_storage::DocumentTorrentStorage>, wt_core::Torrent)>,
    torrent: Option<wt_core::Torrent>,
    /// Go `seekStats.recordStall`: a read attempt timed out and was refreshed.
    on_stall: Option<Arc<dyn Fn() + Send + Sync>>,
    served_unverified: std::collections::HashSet<u32>,
    seen_hash_failures: u64,
}

impl CoreReaderBackend {
    pub fn from_file(file: &File) -> Self {
        let mut inner = file.new_reader();
        inner.set_readahead(PLAYBACK_URGENT_BYTES);
        Self {
            inner,
            stream: None,
            torrent: Some(wt_core::Torrent { inner: file.torrent.clone() }),
            on_stall: None,
            served_unverified: Default::default(),
            seen_hash_failures: 0,
        }
    }

    pub fn from_reader(inner: Reader) -> Self {
        Self {
            inner,
            stream: None,
            torrent: None,
            on_stall: None,
            served_unverified: Default::default(),
            seen_hash_failures: 0,
        }
    }

    pub fn from_file_with_storage(
        file: &File,
        storage: Arc<wt_storage::DocumentTorrentStorage>,
    ) -> Self {
        let mut backend = Self::from_file(file);
        if storage.stream_mode() {
            storage.set_startup_file(file.index());
            backend.stream = Some((
                storage,
                wt_core::Torrent {
                    inner: file.torrent.clone(),
                },
            ));
        }
        backend
    }

    fn core_torrent(&self) -> Option<&wt_core::Torrent> {
        self.torrent.as_ref().or(self.stream.as_ref().map(|(_, torrent)| torrent))
    }

    /// Remember unverified pieces handed to the player.
    fn note_served(&mut self, offset: i64, n: usize) {
        if n == 0 {
            return;
        }
        let Some(torrent) = self.core_torrent().cloned() else { return };
        let Some(piece_length) = torrent.info().map(|info| info.piece_length) else { return };
        let abs = self.inner.file_offset + offset;
        for piece in abs / piece_length..=(abs + n as i64 - 1) / piece_length {
            if !torrent.piece_complete(piece as u32) {
                self.served_unverified.insert(piece as u32);
            }
        }
    }

    /// End the stream if a piece the player already has failed its hash: its
    /// bytes were wrong, and reopening fetches good ones.
    fn check_served_pieces(&mut self) -> io::Result<()> {
        let Some(torrent) = self.core_torrent().cloned() else { return Ok(()) };
        let generation = torrent.inner.hash_fail_generation.load(std::sync::atomic::Ordering::Relaxed);
        if generation == self.seen_hash_failures {
            return Ok(());
        }
        self.seen_hash_failures = generation;
        let failed = torrent.inner.failed_pieces.lock();
        if self.served_unverified.iter().any(|piece| failed.contains(piece)) {
            self.served_unverified.retain(|piece| !failed.contains(piece));
            wt_types::events::record("stream_corrupt", "served piece failed its hash");
            return Err(io::Error::other("served data failed verification"));
        }
        Ok(())
    }

    fn read_available(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        use std::io::{Seek, SeekFrom};
        self.inner.seek(SeekFrom::Start(offset.max(0) as u64))?;
        if let Some((storage, torrent)) = &self.stream {
            use wt_core::reader::{PieceRange, ReaderHost};
            let abs = self.inner.file_offset + offset;
            let piece_length = torrent
                .info()
                .ok_or_else(|| io::Error::other("metadata unavailable"))?
                .piece_length;
            let piece = (abs / piece_length) as u32;
            torrent.update_piece_completion(piece);
            // Publish the existing reader's demand without asking its verified
            // read path to wait. Storage can already have a delivered body block.
            torrent.inner.set_reader_positions(
                self.inner.id(),
                PieceRange {
                    begin: piece,
                    end: ((abs + self.inner.readahead.max(1) + piece_length - 1) / piece_length)
                        .min(torrent.num_pieces() as i64) as u32,
                },
            );
            let waiting_since = std::time::Instant::now();
            let mut last_refresh = waiting_since;
            let seek_mode = torrent.urgent_stealing_enabled();
            let attempts: &[u64] = if seek_mode { &SEEK_READ_ATTEMPTS } else { &READ_ATTEMPTS };
            // Cumulative refresh points before the final attempt.
            let recover_points: Vec<u64> = attempts[..attempts.len() - 1].iter()
                .scan(0, |sum, s| { *sum += s; Some(*sum) }).collect();
            let mut next_recover = 0;
            let mut last_nudge: Option<std::time::Instant> = None;
            let mut armed = None;
            loop {
                if self.inner.cancel.as_ref().is_some_and(|c| c.is_cancelled())
                    || torrent.inner.is_closed()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "playback closed or superseded",
                    ));
                }
                let count = buf.len().min((piece_length - abs % piece_length) as usize);
                let (n, ready, result) = storage.read_verified_stream(&mut buf[..count], abs);
                result.map_err(|e| io::Error::other(e.to_string()))?;
                if n > 0 {
                    return Ok(n);
                }
                if torrent.inner.data_download_disallowed.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "payload transfers are paused"));
                }
                if waiting_since.elapsed() >= std::time::Duration::from_secs(45) {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "playback read timed out"));
                }
                if seek_mode && last_nudge.is_none_or(|at| at.elapsed() >= SEEK_NUDGE_EVERY) {
                    torrent.nudge_urgent_peers();
                    last_nudge = Some(std::time::Instant::now());
                }
                if recover_points.get(next_recover).is_some_and(|&at|
                    waiting_since.elapsed() >= std::time::Duration::from_secs(at)) {
                    torrent.refocus_requests();
                    next_recover += 1;
                    if let Some(f) = &self.on_stall {
                        f();
                    }
                }
                if last_refresh.elapsed() >= std::time::Duration::from_millis(250) {
                    torrent.update_piece_completion(piece);
                    torrent.nudge_urgent_requests();
                    last_refresh = std::time::Instant::now();
                }
                if let Some(ready) = ready {
                    let generation = ready.generation();
                    // Bounded: nothing wakes this wait when a newer seek cancels
                    // it, and that seek cannot start until this read returns.
                    // Arriving blocks do wake it. The generation predates the
                    // read above, so a block landing in between is not missed.
                    ready.wait_timeout(armed.unwrap_or(generation), std::time::Duration::from_millis(5));
                    armed = Some(ready.generation());
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "stream storage returned no bytes",
                    ));
                }
            }
        }
        // Match Go's staged stall recovery rather than spending the entire
        // 45-second budget on an unchanged reader/request window.
        let seek_mode = self.torrent.as_ref().is_some_and(|t| t.urgent_stealing_enabled());
        let attempts: &[u64] = if seek_mode { &SEEK_READ_ATTEMPTS } else { &READ_ATTEMPTS };
        // Go starts the nudger only once a read must wait for data.
        let _nudger = if seek_mode && self.inner.must_wait() {
            self.torrent.clone().and_then(UrgentNudger::start)
        } else {
            None
        };
        for (attempt, &seconds) in attempts.iter().enumerate() {
            let last = attempt + 1 == attempts.len();
            match self.inner.read_timeout(buf, Some(std::time::Duration::from_secs(seconds))) {
                Err(error) if error.kind() == io::ErrorKind::TimedOut && !last => {
                    self.note_stall();
                    self.inner.refresh_demand();
                    if let Some(torrent) = &self.torrent {
                        torrent.refocus_requests();
                        torrent.nudge_urgent_requests();
                    }
                }
                result => return result,
            }
        }
        unreachable!("final playback attempt returns")
    }

    pub fn set_on_stall(&mut self, f: Arc<dyn Fn() + Send + Sync>) {
        self.on_stall = Some(f);
    }

    fn note_stall(&self) {
        if let Some(f) = &self.on_stall {
            f();
        }
    }

    pub fn into_inner(self) -> Reader {
        self.inner
    }
}

impl ReaderBackend for CoreReaderBackend {
    fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize> {
        self.check_served_pieces()?;
        let n = self.read_available(buf, offset)?;
        self.note_served(offset, n);
        Ok(n)
    }

    fn set_readahead(&mut self, bytes: i64) {
        self.inner.set_readahead(bytes);
    }

    fn set_readahead_fn(&mut self, f: Option<ReadaheadCallback>) {
        let mapped: Option<ReadaheadFn> = f.map(|cb| {
            Arc::new(move |ctx: &ReadaheadContext| {
                (cb)(ctx.current_pos, ctx.contiguous_read_start_pos)
            }) as ReadaheadFn
        });
        self.inner.set_readahead_fn(mapped);
    }

    fn set_responsive(&mut self, on: bool) {
        self.inner.set_webtor_responsive(on);
    }

    fn set_cancel(&mut self, cancel: wt_core::CancelToken) {
        self.inner.set_cancel(cancel);
    }

    fn cancel(&mut self) {
        if let Some(cancel) = &self.inner.cancel {
            cancel.cancel();
        }
    }

    fn close(&mut self) {
        self.inner.close();
    }
}

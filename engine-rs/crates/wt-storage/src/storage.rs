use crate::cache_lru::PieceLRU;
use crate::cache_store::RESUME_WINDOW_MAX_BYTES;
use crate::checkpoint::{
    checkpoint_path, delete_piece_checkpoint, file_stable, identity_from_file,
    read_piece_checkpoint, write_piece_checkpoint, FileIdentity, PieceCheckpoint,
};
use crate::extent::{hash_extents, pread_extents, FileSpan};
use crate::pieces::PieceRun;
use crate::ranges::{merge_received_range, ranges_cover, ByteRange};
use crate::sparse::seek_data;
use crate::zero_hash::zero_piece_hash;
use parking_lot::{Condvar, Mutex};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use wt_types::{
    ClientStorage, Completion, Info, InfoHash, Piece, PieceStorage, StorageError, StorageResult,
    TorrentStorage,
};

pub const DEFAULT_MAX_CACHE_BYTES: i64 = 64 * 1024 * 1024;
pub const MAX_RAM_CACHE_BYTES: i64 = 1024 * 1024 * 1024;
pub const MIN_RAM_CACHE_BYTES: i64 = 16 * 1024 * 1024;
pub const DEFAULT_STREAM_CACHE_MAX_BYTES: i64 = 50 * 1024 * 1024;
pub const STREAM_CACHE_KEEP_BYTES: i64 = 24 * 1024 * 1024;
pub const DIRECT_PLAY_MIN_PIECES: i64 = 4;
pub const DIRECT_STARTUP_HEAD: i64 = 2 << 20;
pub const DIRECT_STARTUP_TAIL: i64 = 2 << 20;
pub const DIRECT_STARTUP_BUDGET: i64 = 8 << 20;
pub const DIRECT_STARTUP_TOTAL_BUDGET: i64 = 1 << 30;
pub const DIRECT_PRELOAD_BUDGET: i64 = 8 << 20;
pub const PREVIEW_BUDGET_BYTES: i64 = 8 << 20;
pub const MEMORY_LIMIT_OVERHEAD: i64 = 256 * 1024 * 1024;

/// Test/seam hook replacing `File::sync_all` during checkpoint drain.
pub type FileSyncHook = Arc<dyn Fn(RawFd) -> std::io::Result<()> + Send + Sync>;
/// Test/seam hook replacing checkpoint file persistence during drain.
pub type WriteCheckpointHook =
    Arc<dyn Fn(&Path, &PieceCheckpoint) -> StorageResult<()> + Send + Sync>;

// ---------------------------------------------------------------- StreamReady

#[derive(Clone, Default)]
pub struct StreamReady {
    inner: Arc<(Mutex<u64>, Condvar)>,
}

impl StreamReady {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(0), Condvar::new())),
        }
    }

    pub fn notify(&self) {
        let mut gen = self.inner.0.lock();
        *gen += 1;
        self.inner.1.notify_all();
    }

    pub fn generation(&self) -> u64 {
        *self.inner.0.lock()
    }

    pub fn wait_timeout(&self, cur_gen: u64, timeout: Duration) -> bool {
        let mut gen = self.inner.0.lock();
        if *gen != cur_gen {
            return true;
        }
        !self.inner.1.wait_for(&mut gen, timeout).timed_out()
    }
}

// ---------------------------------------------------------------- FileDescriptor

#[derive(Debug)]
pub struct FileDescriptor {
    pub file: Option<File>,
    pub offset: i64,
    pub length: i64,
    pub path: String,
    pub external: bool,
}

// ---------------------------------------------------------------- DocumentStorageClient

pub struct DocumentStorageClient {
    inner: Mutex<ClientInner>,
    base_dir: PathBuf,
}

struct ClientInner {
    cache_limit_bytes: i64,
    torrents: HashMap<String, Arc<DocumentTorrentStorage>>,
}

impl DocumentStorageClient {
    pub fn new<P: AsRef<Path>>(base_dir: P) -> Self {
        Self {
            inner: Mutex::new(ClientInner {
                cache_limit_bytes: DEFAULT_MAX_CACHE_BYTES,
                torrents: HashMap::new(),
            }),
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    pub fn get_storage(&self, key: &str) -> Option<Arc<DocumentTorrentStorage>> {
        let inner = self.inner.lock();
        inner.torrents.get(key).cloned()
    }

    pub fn cache_limit(&self) -> i64 {
        let inner = self.inner.lock();
        if inner.cache_limit_bytes <= 0 {
            DEFAULT_MAX_CACHE_BYTES
        } else {
            inner.cache_limit_bytes
        }
    }

    pub fn set_cache_limit(&self, bytes: i64) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        inner.cache_limit_bytes = bytes;
        for st in inner.torrents.values() {
            st.set_cache_limit(bytes);
        }
        Ok(())
    }

    pub fn set_memory_limit(&self, bytes: i64) {
        let _ = self.set_cache_limit(bytes);
    }

    pub fn memory_limit(&self) -> i64 {
        let inner = self.inner.lock();
        let mut total = MEMORY_LIMIT_OVERHEAD;
        for st in inner.torrents.values() {
            if inner.cache_limit_bytes > 0 {
                total += inner.cache_limit_bytes;
            } else {
                let cl = st.cache_limit();
                if cl > 0 {
                    total += cl;
                } else {
                    total += DEFAULT_MAX_CACHE_BYTES;
                }
            }
        }
        total
    }

    pub fn delete_checkpoint(&self, info_hash_hex: &str) -> StorageResult<()> {
        let storage = {
            let inner = self.inner.lock();
            inner.torrents.get(info_hash_hex).cloned()
        };
        if let Some(st) = storage {
            st.delete_checkpoint()
        } else {
            delete_piece_checkpoint(&self.base_dir, info_hash_hex)
        }
    }

    pub fn remove_torrent(&self, info_hash: InfoHash) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        inner.torrents.remove(&info_hash.to_hex());
        Ok(())
    }
}

impl ClientStorage for DocumentStorageClient {
    fn open_torrent(
        &self,
        info: &Info,
        info_hash: InfoHash,
    ) -> StorageResult<Arc<dyn TorrentStorage>> {
        let key = info_hash.to_hex();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.torrents.get(&key) {
            if !existing.is_closed() {
                existing.update_info(info);
                return Ok(existing.clone());
            }
        }
        let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash, &self.base_dir);
        if inner.cache_limit_bytes > 0 {
            st.set_cache_limit(inner.cache_limit_bytes);
        }
        inner.torrents.insert(key, st.clone());
        Ok(st)
    }
}

// ---------------------------------------------------------------- DocumentTorrentStorage

pub struct DocumentTorrentStorage {
    inner: Mutex<StorageInner>,
    pub info_hash: InfoHash,
    pub base_dir: PathBuf,
    restore_verify: AtomicBool,
    freeze_trusted: AtomicBool,
    weak_self: Mutex<Weak<DocumentTorrentStorage>>,
    startup_drained: Condvar,
    #[cfg(test)]
    startup_save_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

pub struct StorageInner {
    pub info: Option<Info>,
    pub chunk_length: i64,
    pub total_length: i64,
    pub files: Vec<FileDescriptor>,
    pub attached: bool,
    pub closed: bool,
    pub stream: bool,
    pub needs_verify: bool,
    pub preview_temp: bool,

    // Cache / write-back
    pub cache: HashMap<usize, Vec<u8>>,
    pub cache_dirty: HashMap<usize, bool>,
    pub dirty_spans: HashMap<usize, Vec<ByteRange>>,
    pub partial_slots: HashMap<usize, Vec<ByteRange>>,
    pub saved_partial: HashMap<usize, bool>,
    pub cache_order: PieceLRU,
    pub cache_bytes: i64,
    pub max_cache_bytes: i64,
    pub download_max_bytes: i64,
    pub stream_max_bytes: i64,

    // Completion & checkpoints
    pub completed: HashMap<usize, bool>,
    pub identities: Vec<FileIdentity>,
    pub checkpoint_dirty: bool,
    pub checkpoint_last_err: Option<String>,
    pub flush_err: Option<String>,
    pub pending_sync_fds: HashSet<RawFd>,

    // Direct play
    startup_pending: HashMap<usize, Vec<u8>>,
    startup_in_flight: Option<usize>,
    startup_pending_bytes: usize,
    startup_writing: bool,
    pub startup_pieces: HashMap<usize, bool>,
    pub startup_saved: HashMap<usize, bool>,
    pub startup_on_disk: HashMap<usize, bool>,
    pub startup_order: Vec<usize>,
    pub startup_file: usize,
    pub startup_planned: bool,
    pub direct_selected: Vec<usize>,
    pub preload_pieces: Option<HashMap<usize, bool>>,
    pub preload_file: usize,

    pub stream_demand: HashMap<usize, bool>,
    pub stream_lost: HashMap<usize, bool>,
    pub stream_whole: HashMap<usize, bool>,
    pub stream_ranges: HashMap<usize, Vec<ByteRange>>,
    pub stream_revision: u64,
    pub stream_grid_runs: Vec<PieceRun>,
    pub stream_grid_revision: u64,
    pub stream_grid_total: usize,

    pub stream_watch: HashMap<usize, bool>,
    pub stream_ready: StreamReady,

    pub on_stream_evict: Option<Arc<dyn Fn(usize) + Send + Sync>>,

    /// Bumped on delete/close so in-flight checkpoint publishers abort.
    pub checkpoint_gen: u64,
    /// Backoff after a failed write-back (Go `flushRetry`).
    pub flush_retry: Duration,

    /// Test/seam: override `File::sync_all` during checkpoint drain.
    pub file_sync_hook: Option<FileSyncHook>,
    /// Test/seam: override checkpoint file write during drain.
    pub write_checkpoint_hook: Option<WriteCheckpointHook>,
}

impl DocumentTorrentStorage {
    pub fn new<P: AsRef<Path>>(info: Option<Info>, info_hash: InfoHash, base_dir: P) -> Arc<Self> {
        let stream_ready = StreamReady::new();
        let st = Arc::new(Self {
            inner: Mutex::new(StorageInner {
                info: None,
                chunk_length: 0,
                total_length: 0,
                files: Vec::new(),
                attached: false,
                closed: false,
                stream: false,
                needs_verify: false,
                preview_temp: false,

                cache: HashMap::new(),
                cache_dirty: HashMap::new(),
                dirty_spans: HashMap::new(),
                partial_slots: HashMap::new(),
                saved_partial: HashMap::new(),
                cache_order: PieceLRU::new(),
                cache_bytes: 0,
                max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
                download_max_bytes: DEFAULT_MAX_CACHE_BYTES,
                stream_max_bytes: DEFAULT_STREAM_CACHE_MAX_BYTES,

                completed: HashMap::new(),
                identities: Vec::new(),
                checkpoint_dirty: false,
                checkpoint_last_err: None,
                flush_err: None,
                pending_sync_fds: HashSet::new(),

                startup_pending: HashMap::new(),
                startup_in_flight: None,
                startup_pending_bytes: 0,
                startup_writing: false,
                startup_pieces: HashMap::new(),
                startup_saved: HashMap::new(),
                startup_on_disk: HashMap::new(),
                startup_order: Vec::new(),
                startup_file: 0,
                startup_planned: false,
                direct_selected: Vec::new(),
                preload_pieces: None,
                preload_file: 0,

                stream_demand: HashMap::new(),
                stream_lost: HashMap::new(),
                stream_whole: HashMap::new(),
                stream_ranges: HashMap::new(),
                stream_revision: 0,
                stream_grid_runs: Vec::new(),
                stream_grid_revision: 0,
                stream_grid_total: 0,

                stream_watch: HashMap::new(),
                stream_ready,

                on_stream_evict: None,
                checkpoint_gen: 0,
                flush_retry: Duration::from_millis(0),
                file_sync_hook: None,
                write_checkpoint_hook: None,
            }),
            info_hash,
            base_dir: base_dir.as_ref().to_path_buf(),
            restore_verify: AtomicBool::new(false),
            freeze_trusted: AtomicBool::new(false),
            weak_self: Mutex::new(Weak::new()),
            startup_drained: Condvar::new(),
            #[cfg(test)]
            startup_save_hook: Mutex::new(None),
        });
        *st.weak_self.lock() = Arc::downgrade(&st);
        if let Some(inf) = info {
            st.update_info(&inf);
        }
        st
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    pub fn stream_mode(&self) -> bool {
        let inner = self.inner.lock();
        inner.stream && inner.attached && !inner.preview_temp
    }

    pub fn needs_verify(&self) -> bool {
        self.inner.lock().needs_verify
    }

    pub fn piece_complete(&self, index: usize) -> bool {
        let inner = self.inner.lock();
        inner.completed.get(&index).copied().unwrap_or(false)
    }

    pub fn set_piece_complete(&self, index: usize, complete: bool) {
        let mut inner = self.inner.lock();
        inner.completed.insert(index, complete);
    }

    pub fn piece_at(&self, index: usize) -> Arc<DocumentPiece> {
        let piece = {
            let inner = self.inner.lock();
            let info = inner.info.as_ref().expect("info not set");
            info.piece(index as u32)
        };
        let arc = self.weak_self.lock().upgrade().expect("storage dropped");
        Arc::new(DocumentPiece {
            storage: arc,
            piece,
        })
    }

    pub fn set_restore_verify(&self, enabled: bool) {
        self.restore_verify.store(enabled, Ordering::SeqCst);
    }

    pub fn restore_verify_enabled(&self) -> bool {
        self.restore_verify.load(Ordering::SeqCst)
    }

    pub fn unfreeze_trusted(&self) {
        self.freeze_trusted.store(false, Ordering::SeqCst);
    }

    pub fn checkpoint_persist_error(&self) -> Option<String> {
        self.inner.lock().checkpoint_last_err.clone()
    }

    pub fn write_back_error(&self) -> Option<String> {
        self.inner.lock().flush_err.clone()
    }

    /// Go `flushRetry` — delay applied after a failed write-back.
    pub fn flush_retry(&self) -> Duration {
        self.inner.lock().flush_retry
    }

    pub fn checkpoint_dirty(&self) -> bool {
        let inner = self.inner.lock();
        inner.checkpoint_dirty || !inner.pending_sync_fds.is_empty()
    }

    /// Bytes currently held in the piece RAM cache (Go `cacheBytes`).
    pub fn cache_bytes(&self) -> i64 {
        self.inner.lock().cache_bytes
    }

    /// Current per-torrent RAM cache cap (Go `maxCacheBytes`).
    pub fn max_cache_bytes(&self) -> i64 {
        self.inner.lock().max_cache_bytes
    }

    /// Clear Direct Play startup protection (test helper for tiny-piece fixtures).
    pub fn clear_startup_pieces(&self) {
        let mut inner = self.inner.lock();
        inner.startup_pieces.clear();
        inner.startup_order.clear();
        inner.startup_planned = false;
    }

    /// Assign the stream-completion callback (Go `onStreamEvict`). Invoked
    /// asynchronously after Direct Play RAM eviction and `mark_complete`;
    /// `close` does not join it.
    pub fn set_on_stream_evict(&self, callback: Option<Arc<dyn Fn(usize) + Send + Sync>>) {
        self.inner.lock().on_stream_evict = callback;
    }

    /// Flush every dirty piece under the storage lock (Go `flushAllDirtyLocked`).
    pub fn flush_all_dirty(&self) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        match self.flush_all_dirty_locked(&mut inner) {
            Ok(()) => {
                inner.flush_err = None;
                inner.flush_retry = Duration::from_millis(0);
                Ok(())
            }
            Err(e) => {
                inner.flush_err = Some(e.to_string());
                let next = if inner.flush_retry.is_zero() {
                    Duration::from_millis(100)
                } else {
                    (inner.flush_retry * 2).min(Duration::from_secs(5))
                };
                inner.flush_retry = next;
                Err(e)
            }
        }
    }

    /// Public drain entry (Go `drainSyncAndCheckpoint`).
    /// Releases the storage lock around sync/write hooks so test seams can re-enter.
    pub fn drain_sync_and_checkpoint(&self) {
        // Phase 1: snapshot under lock.
        let (gen, need_cp, pending, sync_hook, write_hook, hash_hex, base) = {
            let mut inner = self.inner.lock();
            if inner.closed || !inner.attached {
                return;
            }
            let _ = self.flush_all_dirty_locked(&mut inner);
            let pending: Vec<RawFd> = inner.pending_sync_fds.drain().collect();
            (
                inner.checkpoint_gen,
                inner.checkpoint_dirty && !inner.stream,
                pending,
                inner.file_sync_hook.clone(),
                inner.write_checkpoint_hook.clone(),
                self.info_hash.to_hex(),
                self.base_dir.clone(),
            )
        };

        // Phase 2: sync without holding the storage lock.
        let mut sync_err: Option<String> = None;
        let mut failed_fds: Vec<RawFd> = Vec::new();
        for fd in &pending {
            let result = if let Some(hook) = &sync_hook {
                hook(*fd)
            } else {
                // Re-lock briefly to sync the live File.
                let inner = self.inner.lock();
                let mut found = false;
                let mut io_err = None;
                for f in &inner.files {
                    if let Some(file) = &f.file {
                        if file.as_raw_fd() == *fd {
                            found = true;
                            if let Err(e) = file.sync_all() {
                                io_err = Some(e);
                            }
                            break;
                        }
                    }
                }
                if !found {
                    Ok(())
                } else if let Some(e) = io_err {
                    Err(e)
                } else {
                    Ok(())
                }
            };
            if let Err(e) = result {
                sync_err = Some(e.to_string());
                failed_fds.push(*fd);
            }
        }

        if let Some(err) = sync_err {
            let mut inner = self.inner.lock();
            for fd in failed_fds {
                inner.pending_sync_fds.insert(fd);
            }
            inner.checkpoint_last_err = Some(err);
            return;
        }

        if !need_cp {
            return;
        }

        // Phase 3: build checkpoint under lock if generation still matches.
        let cp_and_path: Option<(PieceCheckpoint, PathBuf, u64)> = {
            let mut inner = self.inner.lock();
            if inner.closed || !inner.attached || inner.checkpoint_gen != gen {
                return;
            }
            self.capture_identities_locked(&mut inner);
            let Some(info) = inner.info.as_ref() else {
                return;
            };
            let num_pieces = info.num_pieces() as usize;
            let mut complete = vec![false; num_pieces];
            for (&p, &ok) in &inner.completed {
                if p < num_pieces {
                    complete[p] = ok;
                }
            }
            let cp = PieceCheckpoint {
                info_hash: self.info_hash.0,
                piece_length: inner.chunk_length,
                num_pieces,
                files: inner.identities.clone(),
                complete,
            };
            let path = checkpoint_path(&base, &hash_hex);
            let publish_gen = inner.checkpoint_gen;
            Some((cp, path, publish_gen))
        };

        let Some((cp, path, publish_gen)) = cp_and_path else {
            return;
        };

        // Phase 4: persist without holding the lock (hooks may delete/bump gen).
        let write_result = if let Some(hook) = &write_hook {
            hook(&path, &cp)
        } else {
            write_piece_checkpoint(&path, &cp)
        };

        // Phase 5: finalize.
        let mut inner = self.inner.lock();
        if inner.checkpoint_gen != publish_gen {
            let _ = delete_piece_checkpoint(&base, &hash_hex);
            inner.checkpoint_last_err = Some(
                write_result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "checkpoint publish aborted: generation changed".into()),
            );
            return;
        }
        match write_result {
            Ok(()) => {
                inner.checkpoint_dirty = false;
                inner.checkpoint_last_err = None;
            }
            Err(e) => {
                inner.checkpoint_last_err = Some(e.to_string());
            }
        }
    }

    /// Replace the open file for `index` (test seam for read-only write-back failures).
    pub fn replace_file_handle(&self, index: usize, file: File) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        if index >= inner.files.len() {
            return Err(StorageError::Other(format!(
                "file index {index} out of range"
            )));
        }
        inner.files[index].file = Some(file);
        Ok(())
    }

    /// Test seam: override per-file `sync_all` during checkpoint drain.
    pub fn set_file_sync_hook(&self, hook: Option<FileSyncHook>) {
        self.inner.lock().file_sync_hook = hook;
    }

    /// Test seam: override checkpoint persistence during drain.
    pub fn set_write_checkpoint_hook(&self, hook: Option<WriteCheckpointHook>) {
        self.inner.lock().write_checkpoint_hook = hook;
    }

    /// Current checkpoint generation (bumped on delete / close).
    pub fn checkpoint_gen(&self) -> u64 {
        self.inner.lock().checkpoint_gen
    }

    /// Force `checkpoint_dirty` (test helper).
    pub fn set_checkpoint_dirty(&self, dirty: bool) {
        self.inner.lock().checkpoint_dirty = dirty;
    }

    /// Mark a file fd as needing sync (test helper for drain sync seams).
    pub fn note_pending_sync_fd(&self, fd: RawFd) {
        self.inner.lock().pending_sync_fds.insert(fd);
    }

    /// Direct Play grid generation without cloning the compressed piece runs.
    pub fn stream_revision(&self) -> u64 {
        self.inner.lock().stream_revision
    }

    /// Snapshot the half-open piece window using Go `pieceStateLocked` states.
    pub fn piece_grid_states(&self, begin: usize, end: usize) -> Vec<u8> {
        let inner = self.inner.lock();
        (begin..end)
            .map(|index| {
                let in_ram = inner.cache.get(&index).is_some_and(|data| !data.is_empty())
                    && !inner.stream_lost.get(&index).copied().unwrap_or(false);
                if in_ram && inner.completed.get(&index).copied().unwrap_or(false) {
                    b'v'
                } else if inner.startup_saved.get(&index).copied().unwrap_or(false) {
                    b's'
                } else if in_ram
                    && inner
                        .stream_ranges
                        .get(&index)
                        .is_some_and(|ranges| !ranges.is_empty())
                {
                    b'r'
                } else {
                    b'm'
                }
            })
            .collect()
    }

    /// Snapshot Direct Play RAM occupancy as compressed piece runs (Go `streamPieceRuns`).
    pub fn stream_piece_runs(&self, total: usize) -> StorageResult<Vec<PieceRun>> {
        if total > u32::MAX as usize {
            return Err(StorageError::Other("invalid piece count".into()));
        }
        let mut inner = self.inner.lock();
        if !inner.stream_grid_runs.is_empty()
            && inner.stream_grid_revision == inner.stream_revision
            && inner.stream_grid_total == total
        {
            return Ok(inner.stream_grid_runs.clone());
        }

        let mut indices: Vec<usize> = if inner.closed {
            Vec::new()
        } else {
            inner
                .cache
                .iter()
                .filter(|(&index, data)| index < total && !data.is_empty())
                .map(|(&index, _)| index)
                .collect()
        };
        indices.sort_unstable();

        let mut runs: Vec<PieceRun> = Vec::with_capacity(2 * indices.len() + 1);
        let mut append_run = |length: usize, flags: u32| {
            if length == 0 {
                return;
            }
            if let Some(last) = runs.last_mut() {
                if last.flags == flags {
                    last.length += length as u32;
                    return;
                }
            }
            runs.push(PieceRun {
                length: length as u32,
                flags,
            });
        };

        let mut next = 0usize;
        for &index in &indices {
            append_run(index - next, 0);
            let flags = if inner.stream_lost.get(&index).copied().unwrap_or(false) {
                0
            } else if inner.completed.get(&index).copied().unwrap_or(false) {
                1
            } else if inner
                .stream_ranges
                .get(&index)
                .map(|r| !r.is_empty())
                .unwrap_or(false)
                && inner.stream_demand.get(&index).copied().unwrap_or(false)
            {
                2
            } else {
                0
            };
            append_run(1, flags);
            next = index + 1;
        }
        append_run(total.saturating_sub(next), 0);

        inner.stream_grid_runs = runs.clone();
        inner.stream_grid_revision = inner.stream_revision;
        inner.stream_grid_total = total;
        Ok(runs)
    }

    pub fn mark_verified(&self) {
        let mut inner = self.inner.lock();
        inner.needs_verify = false;
        inner.checkpoint_dirty = true;
        self.drain_sync_and_checkpoint_locked(&mut inner);
    }

    pub fn set_cache_limit(&self, bytes: i64) {
        let mut inner = self.inner.lock();
        inner.download_max_bytes = bytes;
        inner.stream_max_bytes = bytes;
        if !inner.preview_temp {
            inner.max_cache_bytes = bytes;
            if !inner.closed && inner.attached && inner.stream {
                let min_req = DIRECT_PLAY_MIN_PIECES * inner.chunk_length;
                inner.max_cache_bytes = inner.max_cache_bytes.max(min_req);
            }
        }
    }

    pub fn set_memory_limit(&self, bytes: i64) {
        self.set_cache_limit(bytes);
    }

    pub fn cache_limit(&self) -> i64 {
        self.inner.lock().max_cache_bytes
    }

    pub fn update_info(&self, info: &Info) {
        let mut inner = self.inner.lock();
        if inner.info.is_some() || inner.closed {
            return;
        }
        inner.chunk_length = info.piece_length;
        inner.total_length = info.total_length();
        let mut offset = 0i64;
        let mut files = Vec::with_capacity(info.files.len());
        for f in &info.files {
            files.push(FileDescriptor {
                file: None,
                offset,
                length: f.length,
                path: f.path.join("/"),
                external: false,
            });
            offset += f.length;
        }
        inner.files = files;
        inner.info = Some(info.clone());
    }

    pub fn delete_checkpoint(&self) -> StorageResult<()> {
        {
            let mut inner = self.inner.lock();
            inner.checkpoint_dirty = false;
            inner.identities.clear();
            inner.checkpoint_last_err = None;
            inner.checkpoint_gen = inner.checkpoint_gen.wrapping_add(1);
        }
        delete_piece_checkpoint(&self.base_dir, &self.info_hash.to_hex())
    }

    pub fn selection_can_use_existing_files(&self, selected: &[usize]) -> bool {
        let inner = self.inner.lock();
        if !inner.attached {
            return false;
        }
        for &i in selected {
            if i >= inner.files.len() || inner.files[i].file.is_none() {
                return false;
            }
            if !inner.stream && !inner.files[i].external {
                return false;
            }
        }
        true
    }

    pub fn range_complete(&self, abs_start: i64, length: i64) -> bool {
        let inner = self.inner.lock();
        if inner.closed
            || !inner.attached
            || inner.chunk_length <= 0
            || length <= 0
            || inner.info.is_none()
        {
            return false;
        }
        let begin = (abs_start / inner.chunk_length) as usize;
        let last = ((abs_start + length - 1) / inner.chunk_length) as usize;
        let num_pieces = inner.info.as_ref().unwrap().num_pieces() as usize;
        if last >= num_pieces {
            return false;
        }
        for p in begin..=last {
            if !inner.completed.get(&p).copied().unwrap_or(false) {
                return false;
            }
        }
        true
    }

    pub fn all_selected_complete(&self, selected: &[usize]) -> bool {
        let inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.info.is_none() || inner.chunk_length <= 0 {
            return false;
        }
        let num_pieces = inner.info.as_ref().unwrap().num_pieces() as usize;
        for &index in selected {
            if index >= inner.files.len() {
                return false;
            }
            let f = &inner.files[index];
            if f.length <= 0 {
                continue;
            }
            let begin = (f.offset / inner.chunk_length) as usize;
            let last = (((f.offset + f.length - 1) / inner.chunk_length) as usize)
                .min(num_pieces.saturating_sub(1));
            for p in begin..=last {
                if !inner.completed.get(&p).copied().unwrap_or(false) {
                    return false;
                }
            }
        }
        true
    }

    // --- Attachment & Selection

    pub fn attach_descriptors(
        &self,
        descriptors: &[Option<RawFd>],
        selected: &[usize],
    ) -> StorageResult<()> {
        self.attach_descriptors_mode(descriptors, selected, false)
    }

    pub fn attach_descriptors_mode(
        &self,
        descriptors: &[Option<RawFd>],
        selected: &[usize],
        stream: bool,
    ) -> StorageResult<()> {
        if stream {
            return self.attach_direct_play(descriptors, selected);
        }
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(StorageError::Closed);
        }
        if inner.info.is_none() {
            return Err(StorageError::Other("metadata not ready".into()));
        }
        if inner.attached {
            return Err(StorageError::Other("storage already configured".into()));
        }
        if descriptors.len() != inner.files.len() {
            return Err(StorageError::Other("descriptors count mismatch".into()));
        }

        let mut chosen = HashSet::new();
        for &i in selected {
            if i >= inner.files.len() || !chosen.insert(i) {
                return Err(StorageError::Other(format!("invalid selected index {}", i)));
            }
        }

        let mut opened: Vec<Option<File>> = Vec::with_capacity(inner.files.len());
        let mut pre_identities = vec![FileIdentity::default(); inner.files.len()];

        for i in 0..inner.files.len() {
            let f_len = inner.files[i].length;
            let f_path = inner.files[i].path.clone();

            let file = if let Some(fd) = descriptors[i] {
                if fd < 0 {
                    return Err(StorageError::Other(format!("invalid fd for file {}", i)));
                }
                let dup_fd = unsafe { libc::dup(fd) };
                if dup_fd < 0 {
                    return Err(StorageError::Io(std::io::Error::last_os_error()));
                }
                let f = unsafe { File::from_raw_fd(dup_fd) };
                let meta = f.metadata()?;
                if meta.len() > 0 {
                    inner.needs_verify = true;
                }
                let (id, ok) = identity_from_file(&f, true, f_len, f_path);
                if ok && meta.len() as i64 == f_len {
                    pre_identities[i] = id;
                }
                f.set_len(f_len as u64)?;
                f
            } else {
                let part_path =
                    engine_owned_part_path(&self.base_dir, &self.info_hash.to_hex(), i, false);
                if let Some(p) = part_path.parent() {
                    fs::create_dir_all(p)?;
                }
                let f = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&part_path)?;
                let meta = f.metadata()?;
                let (id, ok) = identity_from_file(&f, false, f_len, f_path);
                if ok && meta.len() as i64 == f_len {
                    pre_identities[i] = id;
                }
                f.set_len(f_len as u64)?;
                f
            };
            opened.push(Some(file));
        }

        for (i, f) in opened.into_iter().enumerate() {
            inner.files[i].file = f;
            inner.files[i].external = descriptors[i].is_some();
        }
        inner.stream = false;
        inner.attached = true;

        // Flush prepared cache data to disk
        let prepared_spans = inner.dirty_spans.clone();
        let cache_items: Vec<(usize, Vec<u8>)> =
            inner.cache.iter().map(|(&k, v)| (k, v.clone())).collect();
        for (index, data) in cache_items {
            let spans = prepared_spans.get(&index).cloned().unwrap_or_else(|| {
                vec![ByteRange {
                    start: 0,
                    end: data.len() as i64,
                }]
            });
            for span in spans {
                let start = (index as i64) * inner.chunk_length + span.start;
                let chunk = &data[span.start as usize..span.end as usize];
                Self::write_to_files_locked(&mut inner.files, start, chunk)?;
            }
        }
        inner.cache_dirty.clear();
        inner.dirty_spans.clear();

        // Check if cache exceeded maxCacheBytes
        if inner.cache_bytes > inner.max_cache_bytes {
            inner.cache.clear();
            inner.cache_bytes = 0;
            inner.cache_order.reset();
        }

        // Set partial slots for prepared data
        for (index, spans) in prepared_spans {
            if let Some(data) = inner.cache.get(&index) {
                if !ranges_cover(&spans, 0, data.len() as i64) {
                    inner.partial_slots.insert(index, spans);
                    inner.saved_partial.insert(index, true);
                }
            }
        }

        // Checkpoints
        let loaded = self.load_checkpoint_locked(&mut inner, &pre_identities);
        self.capture_identities_locked(&mut inner);
        if loaded {
            inner.checkpoint_dirty = true;
            self.freeze_trusted.store(true, Ordering::SeqCst);
        }

        Ok(())
    }

    pub fn attach_direct_play(
        &self,
        descriptors: &[Option<RawFd>],
        selected: &[usize],
    ) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        if inner.closed || inner.info.is_none() {
            return Err(StorageError::Other("storage closed or missing info".into()));
        }
        if inner.preview_temp {
            self.release_preview_temp_locked(&mut inner);
        }
        if inner.attached {
            return Err(StorageError::Other("storage already attached".into()));
        }
        if descriptors.len() != inner.files.len() {
            return Err(StorageError::Other(
                "file descriptors do not match torrent files".into(),
            ));
        }
        for (i, fd) in descriptors.iter().enumerate() {
            if fd.is_some() {
                return Err(StorageError::Other(format!(
                    "direct play rejects external descriptors for file {}",
                    i
                )));
            }
        }

        let mut chosen = HashSet::new();
        for &i in selected {
            if i >= inner.files.len() || !chosen.insert(i) {
                return Err(StorageError::Other(format!("invalid selected index {}", i)));
            }
        }

        inner.stream = true;
        inner.attached = true;
        inner.max_cache_bytes = inner.stream_max_bytes;
        if inner.max_cache_bytes <= 0 {
            inner.max_cache_bytes = DEFAULT_STREAM_CACHE_MAX_BYTES;
        }
        let min_req = DIRECT_PLAY_MIN_PIECES * inner.chunk_length;
        if min_req > MAX_RAM_CACHE_BYTES {
            inner.stream = false;
            inner.attached = false;
            return Err(StorageError::Other(
                "piece exceeds direct play memory budget; use Download".into(),
            ));
        }
        inner.max_cache_bytes = inner.max_cache_bytes.max(min_req);

        inner.direct_selected = selected.to_vec();
        inner.startup_order = self.startup_plan_locked(&inner, None);
        inner.startup_planned = false;
        inner.startup_pieces.clear();
        inner.startup_saved.clear();
        inner.startup_on_disk.clear();
        inner.stream_demand.clear();
        inner.stream_lost.clear();
        inner.stream_whole.clear();
        inner.stream_ranges.clear();
        inner.stream_watch.clear();

        let mut quick = HashSet::new();
        let mut quick_bytes = 0i64;
        let startup_order = inner.startup_order.clone();
        for &p in &startup_order {
            inner.startup_pieces.insert(p, true);
            let p_len = inner.info.as_ref().unwrap().piece(p as u32).length;
            if quick_bytes + p_len <= DIRECT_STARTUP_BUDGET {
                quick.insert(p);
                quick_bytes += p_len;
            } else if self.startup_file_on_disk_locked(&inner, p) {
                inner.startup_on_disk.insert(p, true);
            }
        }

        self.validate_startup_pieces_locked(&mut inner, &quick);

        inner.cache.clear();
        inner.cache_dirty.clear();
        inner.cache_bytes = 0;
        inner.dirty_spans.clear();
        inner.partial_slots.clear();
        inner.saved_partial.clear();
        inner.cache_order.reset();
        inner.stream_revision += 1;

        Ok(())
    }

    pub fn update_selection(
        &self,
        descriptors: &[Option<RawFd>],
        selected: &[usize],
    ) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.info.is_none() {
            return Err(StorageError::Other(
                "storage not in valid state for selection".into(),
            ));
        }
        if descriptors.len() != inner.files.len() {
            return Err(StorageError::Other("descriptors length mismatch".into()));
        }

        let mut chosen = HashSet::new();
        for &i in selected {
            if i >= inner.files.len() || !chosen.insert(i) {
                return Err(StorageError::Other(format!("invalid selected index {}", i)));
            }
        }

        let old_files: Vec<FileDescriptor> = std::mem::take(&mut inner.files);
        let mut next_files = Vec::with_capacity(old_files.len());
        let mut opened_replacements: Vec<(usize, File)> = Vec::new();

        for i in 0..old_files.len() {
            let is_sel = chosen.contains(&i);
            let cur_external = old_files[i].external;

            if !is_sel || (cur_external || inner.stream) && descriptors[i].is_none() {
                next_files.push(FileDescriptor {
                    file: None,
                    offset: old_files[i].offset,
                    length: old_files[i].length,
                    path: old_files[i].path.clone(),
                    external: old_files[i].external,
                });
                continue;
            }

            if inner.stream {
                return Err(StorageError::Other(format!(
                    "stream storage rejects external descriptor for file {}",
                    i
                )));
            }
            let fd = match descriptors[i] {
                Some(f) if f >= 0 => f,
                _ => {
                    return Err(StorageError::Other(format!(
                        "descriptor required for newly selected file {}",
                        i
                    )))
                }
            };

            let dup_fd = unsafe { libc::dup(fd) };
            if dup_fd < 0 {
                return Err(StorageError::Io(std::io::Error::last_os_error()));
            }
            let file = unsafe { File::from_raw_fd(dup_fd) };
            file.set_len(0)?;
            file.set_len(old_files[i].length as u64)?;

            opened_replacements.push((i, file));
            next_files.push(FileDescriptor {
                file: None,
                offset: old_files[i].offset,
                length: old_files[i].length,
                path: old_files[i].path.clone(),
                external: true,
            });
        }

        for (i, f) in old_files.into_iter().enumerate() {
            if next_files[i].file.is_none() {
                next_files[i].file = f.file;
            }
        }
        for (i, f) in opened_replacements {
            for (&p, &is_comp) in &inner.completed {
                if !is_comp {
                    continue;
                }
                let piece_len = inner.info.as_ref().unwrap().piece(p as u32).length;
                let piece_start = (p as i64) * inner.chunk_length;
                let from = piece_start.max(next_files[i].offset);
                let to = (piece_start + piece_len).min(next_files[i].offset + next_files[i].length);
                if from < to {
                    let mut data = vec![0u8; (to - from) as usize];
                    Self::read_from_files_locked(&next_files, from, &mut data)?;
                    f.write_all_at(&data, (from - next_files[i].offset) as u64)?;
                }
            }
            next_files[i].file = Some(f);
        }

        inner.files = next_files;
        self.capture_identities_locked(&mut inner);
        inner.checkpoint_dirty = true;
        self.drain_sync_and_checkpoint_locked(&mut inner);

        Ok(())
    }

    // --- Piece spans & Extents

    pub fn piece_spans(&self, piece: Piece) -> Option<Vec<FileSpan>> {
        let inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.chunk_length <= 0 {
            return None;
        }
        let start = (piece.index as i64) * inner.chunk_length;
        self.byte_spans_locked(&inner, start, start + piece.length)
    }

    pub fn byte_spans(&self, abs_start: i64, length: i64) -> Option<Vec<FileSpan>> {
        let inner = self.inner.lock();
        if inner.closed || !inner.attached || length <= 0 {
            return None;
        }
        self.byte_spans_locked(&inner, abs_start, abs_start + length)
    }

    fn byte_spans_locked(
        &self,
        inner: &StorageInner,
        start: i64,
        end: i64,
    ) -> Option<Vec<FileSpan>> {
        let mut spans = Vec::new();
        for f in &inner.files {
            if f.offset + f.length <= start || f.offset >= end {
                continue;
            }
            let from = start.max(f.offset);
            let to = end.min(f.offset + f.length);
            if from >= to {
                continue;
            }
            let file = f.file.as_ref()?;
            spans.push(FileSpan {
                fd: file.as_raw_fd(),
                offset: from - f.offset,
                length: to - from,
            });
        }
        if spans.is_empty() {
            None
        } else {
            Some(spans)
        }
    }

    pub fn piece_extent_all_holes(&self, piece: Piece) -> (bool, bool) {
        let inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.chunk_length <= 0 {
            return (false, false);
        }
        if inner.cache.contains_key(&(piece.index as usize)) {
            return (false, true);
        }
        let start = (piece.index as i64) * inner.chunk_length;
        let end = start + piece.length;
        if end <= start {
            return (false, false);
        }

        let mut saw_span = false;
        for f in &inner.files {
            let from = start.max(f.offset);
            let to = end.min(f.offset + f.length);
            if from >= to {
                continue;
            }
            let file = match f.file.as_ref() {
                Some(file) => file,
                None => return (false, false),
            };
            saw_span = true;
            let file_start = from - f.offset;
            let file_end = to - f.offset;

            match seek_data(file.as_raw_fd(), file_start) {
                Ok(Some(pos)) => {
                    if pos < file_end {
                        return (false, true);
                    }
                }
                Ok(None) => {
                    continue;
                }
                Err(_) => return (false, false),
            }
        }
        if !saw_span {
            (false, false)
        } else {
            (true, true)
        }
    }

    // --- Direct Play Specific

    fn startup_plan_locked(&self, inner: &StorageInner, first_file: Option<usize>) -> Vec<usize> {
        let mut files = inner.direct_selected.clone();
        files.sort_unstable();
        if let Some(first) = first_file {
            if let Some(pos) = files.iter().position(|&x| x == first) {
                if pos > 0 {
                    let mut rotated = files[pos..].to_vec();
                    rotated.extend_from_slice(&files[..pos]);
                    files = rotated;
                }
            }
        }

        let mut seen = HashSet::new();
        let mut order = Vec::new();
        let mut budget = 0i64;
        let info = match inner.info.as_ref() {
            Some(i) => i,
            None => return order,
        };

        for &i in &files {
            if i >= inner.files.len() {
                continue;
            }
            let name = &inner.files[i].path;
            if Some(i) != first_file && !is_video_file_name(name) {
                continue;
            }
            for p in startup_file_pieces(&inner.files[i], inner.chunk_length) {
                if seen.insert(p) {
                    let piece_len = info.piece(p as u32).length;
                    if budget + piece_len > DIRECT_STARTUP_TOTAL_BUDGET {
                        return order;
                    }
                    budget += piece_len;
                    order.push(p);
                }
            }
        }
        order
    }

    fn startup_file_on_disk_locked(&self, inner: &StorageInner, index: usize) -> bool {
        let path = self.startup_piece_path(index);
        let expected = inner
            .info
            .as_ref()
            .map(|i| i.piece(index as u32).length + 32)
            .unwrap_or(0);
        fs::metadata(path)
            .map(|m| m.len() as i64 == expected)
            .unwrap_or(false)
    }

    pub fn startup_piece_path(&self, index: usize) -> PathBuf {
        self.base_dir
            .join(".startup")
            .join(self.info_hash.to_hex())
            .join(format!("{}.piece", index))
    }

    fn validate_startup_pieces_locked(&self, inner: &mut StorageInner, planned: &HashSet<usize>) {
        let mut valid = Vec::new();
        for &p in planned {
            if let Ok(data) = self.read_startup_piece_locked(inner, p) {
                if self.valid_startup_piece_locked(inner, p, &data) {
                    valid.push(p);
                } else {
                    let _ = fs::remove_file(self.startup_piece_path(p));
                }
            }
        }
        for &p in planned {
            inner.startup_on_disk.remove(&p);
        }
        for p in valid {
            inner.cache.remove(&p);
            inner.stream_ranges.remove(&p);
            inner.stream_whole.remove(&p);
            inner.stream_lost.remove(&p);
            inner.startup_saved.insert(p, true);
            inner.completed.insert(p, true);
        }
        inner.stream_ready.notify();
    }

    fn read_startup_piece_locked(
        &self,
        inner: &StorageInner,
        index: usize,
    ) -> StorageResult<Vec<u8>> {
        let path = self.startup_piece_path(index);
        let mut file = File::open(path)?;
        let expected = inner
            .info
            .as_ref()
            .ok_or_else(|| StorageError::Other("missing info".into()))?
            .piece(index as u32)
            .length
            + 32;
        let meta = file.metadata()?;
        if meta.len() as i64 != expected {
            return Err(StorageError::Other("invalid startup piece length".into()));
        }
        let mut data = Vec::with_capacity(expected as usize);
        file.read_to_end(&mut data)?;
        Ok(data)
    }

    fn valid_startup_piece_locked(&self, inner: &StorageInner, index: usize, data: &[u8]) -> bool {
        let info = match inner.info.as_ref() {
            Some(i) => i,
            None => return false,
        };
        let expected_len = (info.piece(index as u32).length + 32) as usize;
        if data.len() != expected_len {
            return false;
        }
        let payload = &data[32..];
        let mut sha2 = Sha256::new();
        sha2.update(payload);
        let sum2 = sha2.finalize();
        if sum2.as_slice() != &data[..32] {
            return false;
        }
        if let Some(expected_v1) = info.piece_hash_v1(index as u32) {
            let mut sha1 = Sha1::new();
            sha1.update(payload);
            let sum1 = sha1.finalize();
            if sum1.as_slice() != expected_v1 {
                return false;
            }
        }
        true
    }

    pub fn set_startup_file(&self, index: usize) {
        let mut inner = self.inner.lock();
        if !inner.stream || inner.closed || index >= inner.files.len() {
            return;
        }
        if inner.startup_planned && inner.startup_file == index {
            return;
        }
        inner.startup_file = index;
        inner.startup_planned = true;
        inner.startup_order = self.startup_plan_locked(&inner, Some(index));

        let mut next = HashSet::new();
        for &p in &inner.startup_order {
            next.insert(p);
        }
        if let Some(preload) = &inner.preload_pieces {
            for &p in preload.keys() {
                next.insert(p);
            }
        }
        let to_remove: Vec<usize> = inner
            .startup_pieces
            .keys()
            .filter(|p| !next.contains(p))
            .copied()
            .collect();
        for p in to_remove {
            inner.startup_saved.remove(&p);
            inner.startup_on_disk.remove(&p);
            let _ = fs::remove_file(self.startup_piece_path(p));
            if !inner.cache.contains_key(&p) {
                inner.completed.remove(&p);
            }
        }
        inner.startup_pieces = next.iter().map(|&p| (p, true)).collect();

        let mut missing = HashSet::new();
        for p in startup_file_pieces(&inner.files[index], inner.chunk_length) {
            if next.contains(&p)
                && !inner.startup_saved.contains_key(&p)
                && !inner.completed.contains_key(&p)
            {
                missing.insert(p);
            }
        }
        if !missing.is_empty() {
            self.validate_startup_pieces_locked(&mut inner, &missing);
        }
    }

    pub fn set_preload_file(&self, index: usize) -> HashMap<usize, bool> {
        let mut inner = self.inner.lock();
        inner.preload_pieces = None;
        if !inner.stream || inner.closed || inner.preview_temp || index >= inner.files.len() {
            return HashMap::new();
        }
        let f = &inner.files[index];
        let mut planned = HashMap::new();
        let mut budget = 0i64;
        let pieces = startup_file_pieces(f, inner.chunk_length);
        let info = inner.info.as_ref().unwrap().clone();
        for p in pieces {
            if planned.contains_key(&p) {
                continue;
            }
            if !inner.startup_pieces.contains_key(&p) {
                let p_len = info.piece(p as u32).length;
                if budget + p_len > DIRECT_PRELOAD_BUDGET {
                    continue;
                }
                budget += p_len;
            }
            planned.insert(p, true);
        }

        inner.preload_file = index;
        inner.preload_pieces = Some(planned.clone());

        let mut missing = HashSet::new();
        for &p in planned.keys() {
            inner.startup_pieces.insert(p, true);
            if !inner.startup_saved.contains_key(&p) && !inner.completed.contains_key(&p) {
                missing.insert(p);
            }
        }
        if !missing.is_empty() {
            self.validate_startup_pieces_locked(&mut inner, &missing);
        }
        planned
    }

    pub fn startup_fetch_pieces(&self, playing: bool) -> Vec<usize> {
        let inner = self.inner.lock();
        if !inner.stream || inner.closed || inner.preview_temp {
            return Vec::new();
        }
        let limit = if playing {
            inner.max_cache_bytes / 20
        } else {
            inner.max_cache_bytes / 4
        };
        let mut used = 0i64;
        let info = match inner.info.as_ref() {
            Some(i) => i,
            None => return Vec::new(),
        };

        for &p in &inner.startup_order {
            if inner.completed.get(&p).copied().unwrap_or(false)
                && !inner.startup_saved.get(&p).copied().unwrap_or(false)
                && inner.cache.contains_key(&p)
            {
                used += info.piece(p as u32).length;
            }
        }
        let mut out = Vec::new();
        for &p in &inner.startup_order {
            if inner.completed.get(&p).copied().unwrap_or(false)
                || inner.startup_saved.get(&p).copied().unwrap_or(false)
                || inner.startup_on_disk.get(&p).copied().unwrap_or(false)
            {
                continue;
            }
            let n = info.piece(p as u32).length;
            if (!out.is_empty() || used > 0) && used + n > limit {
                break;
            }
            out.push(p);
            used += n;
        }
        out
    }

    pub fn read_verified_stream(
        &self,
        buf: &mut [u8],
        abs: i64,
    ) -> (usize, Option<StreamReady>, StorageResult<()>) {
        let mut inner = self.inner.lock();
        if inner.closed || !inner.stream || !inner.attached {
            return (0, None, Err(StorageError::Closed));
        }
        if inner.chunk_length <= 0 {
            return (
                0,
                None,
                Err(StorageError::Other("chunk length zero".into())),
            );
        }
        let index = (abs / inner.chunk_length) as usize;
        let is_comp = inner.completed.get(&index).copied().unwrap_or(false);

        let chunk_off = abs % inner.chunk_length;
        if !is_comp {
            if let Some(n) = self.read_received_locked(&mut inner, buf, index, chunk_off) {
                if n > 0 {
                    return (n, None, Ok(()));
                }
            }
            inner.stream_watch.insert(index, true);
            let ready = inner.stream_ready.clone();
            return (0, Some(ready), Ok(()));
        }

        let chunk_len = inner.chunk_length;
        match self.read_piece_data_locked(&mut inner, index, abs % chunk_len, buf) {
            Ok(n) => (n, None, Ok(())),
            Err(e) => (0, None, Err(e)),
        }
    }

    fn read_received_locked(
        &self,
        inner: &mut StorageInner,
        buf: &mut [u8],
        index: usize,
        off: i64,
    ) -> Option<usize> {
        if inner.stream_lost.get(&index).copied().unwrap_or(false)
            || inner.startup_pieces.get(&index).copied().unwrap_or(false)
            || inner
                .preload_pieces
                .as_ref()
                .map(|p| p.contains_key(&index))
                .unwrap_or(false)
        {
            return None;
        }
        let data = inner.cache.get(&index)?;
        let ranges = inner.stream_ranges.get(&index)?;
        for r in ranges {
            if r.start <= off && off < r.end {
                let n = (r.end - off).min(buf.len() as i64) as usize;
                buf[..n].copy_from_slice(&data[off as usize..off as usize + n]);
                inner.cache_order.touch(index as i32);
                return Some(n);
            }
        }
        None
    }

    pub fn inject_verified_bytes(&self, abs: i64, data: &[u8]) -> bool {
        let mut inner = self.inner.lock();
        if inner.closed
            || !inner.attached
            || !inner.stream
            || inner.chunk_length <= 0
            || data.is_empty()
        {
            return false;
        }
        let info = match inner.info.clone() {
            Some(i) => i,
            None => return false,
        };
        if abs < 0 || abs + data.len() as i64 > info.total_length() {
            return false;
        }

        let end = abs + data.len() as i64;
        let begin = (abs / inner.chunk_length) as usize;
        let last = ((end - 1) / inner.chunk_length) as usize;
        let mut installed = false;
        let mut pending_refresh = Vec::new();

        for index in begin..=last {
            let piece = info.piece(index as u32);
            let piece_start = (index as i64) * inner.chunk_length;
            let piece_end = piece_start + piece.length;
            if abs > piece_start || end < piece_end {
                continue;
            }
            if inner.completed.get(&index).copied().unwrap_or(false)
                && inner.cache.get(&index).map(|d| d.len() as i64) == Some(piece.length)
            {
                continue;
            }
            let off = (piece_start - abs) as usize;
            let buf = &data[off..off + piece.length as usize];
            if let Some(exp_v1) = info.piece_hash_v1(index as u32) {
                let mut sha1 = Sha1::new();
                sha1.update(buf);
                if sha1.finalize().as_slice() != exp_v1 {
                    continue;
                }
            }

            let Ok((_created, refresh)) =
                self.ensure_stream_slot_locked(&mut inner, index, piece.length)
            else {
                continue;
            };
            pending_refresh.extend(refresh);
            if let Some(cache_slot) = inner.cache.get_mut(&index) {
                cache_slot.copy_from_slice(buf);
            }
            inner.cache_dirty.insert(index, false);
            inner.partial_slots.remove(&index);
            inner.completed.insert(index, true);
            inner.stream_lost.remove(&index);
            inner.stream_whole.insert(index, true);
            inner.stream_ranges.insert(
                index,
                vec![ByteRange {
                    start: 0,
                    end: piece.length,
                }],
            );
            inner.cache_order.touch(index as i32);
            installed = true;
        }

        if installed {
            inner.stream_ready.notify();
        }
        let callback = inner.on_stream_evict.clone();
        drop(inner);
        if let Some(cb) = callback {
            for v in pending_refresh {
                let cb = Arc::clone(&cb);
                std::thread::spawn(move || cb(v));
            }
        }
        installed
    }

    pub fn copy_verified_range(&self, abs: i64, length: i64) -> Option<Vec<u8>> {
        if length <= 0 || length > RESUME_WINDOW_MAX_BYTES {
            return None;
        }
        if !self.range_complete(abs, length) {
            return None;
        }
        let mut buf = vec![0u8; length as usize];
        if let Some(spans) = self.byte_spans(abs, length) {
            if let Ok(n) = pread_extents(&spans, &mut buf) {
                if n as i64 == length {
                    return Some(buf);
                }
            }
        }

        let inner = self.inner.lock();
        let mut n = 0usize;
        let info = inner.info.as_ref()?.clone();
        while n < buf.len() {
            let cur_abs = abs + n as i64;
            let index = (cur_abs / inner.chunk_length) as usize;
            if !inner.completed.get(&index).copied().unwrap_or(false) {
                return None;
            }
            let data = inner.cache.get(&index)?;
            let piece_off = (cur_abs % inner.chunk_length) as usize;
            let p_len = info.piece(index as u32).length as usize;
            let want = (buf.len() - n).min(p_len - piece_off);
            buf[n..n + want].copy_from_slice(&data[piece_off..piece_off + want]);
            n += want;
        }
        Some(buf)
    }

    pub fn set_stream_demand(&self, demand: HashMap<usize, bool>) {
        let mut inner = self.inner.lock();
        if inner.stream {
            if inner.stream_demand != demand {
                inner.stream_revision += 1;
            }
            inner.stream_demand = demand;
        }
    }

    // --- Preview

    pub fn attach_preview_temp(&self, file_index: usize) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(StorageError::Closed);
        }
        if inner.info.is_none() {
            return Err(StorageError::Other("metadata not ready".into()));
        }
        if inner.attached {
            return Ok(());
        }
        if file_index >= inner.files.len() {
            return Err(StorageError::Other("invalid preview file index".into()));
        }

        let dir = self.base_dir.join(".preview").join(self.info_hash.to_hex());
        fs::create_dir_all(&dir)?;

        inner.stream = true;
        inner.attached = true;
        inner.preview_temp = true;
        inner.max_cache_bytes = PREVIEW_BUDGET_BYTES;
        inner.startup_pieces.clear();
        inner.startup_saved.clear();
        inner.stream_demand.clear();
        inner.stream_lost.clear();
        inner.stream_whole.clear();
        inner.stream_ranges.clear();

        let f = &inner.files[file_index];
        let mut budget = 0i64;
        let pieces = startup_file_pieces(f, inner.chunk_length);
        let info = inner.info.as_ref().unwrap().clone();
        for p in pieces {
            let n = info.piece(p as u32).length;
            if budget + n > PREVIEW_BUDGET_BYTES {
                break;
            }
            inner.startup_pieces.insert(p, true);
            budget += n;
        }
        Ok(())
    }

    pub fn release_preview_temp(&self) {
        let mut inner = self.inner.lock();
        self.release_preview_temp_locked(&mut inner);
    }

    fn release_preview_temp_locked(&self, inner: &mut StorageInner) {
        if !inner.preview_temp {
            return;
        }
        inner.preview_temp = false;
        inner.max_cache_bytes = inner.download_max_bytes;
        inner.attached = false;
        inner.stream = false;
        inner.cache.clear();
        inner.cache_dirty.clear();
        inner.cache_bytes = 0;
        inner.dirty_spans.clear();
        inner.partial_slots.clear();
        inner.saved_partial.clear();
        inner.cache_order.reset();
        inner.completed.clear();
        inner.startup_pieces.clear();
        inner.startup_saved.clear();
        inner.stream_demand.clear();
        inner.stream_lost.clear();
        inner.stream_whole.clear();
        inner.stream_ranges.clear();
        inner.stream_ready.notify();
        let dir = self.base_dir.join(".preview").join(self.info_hash.to_hex());
        let _ = fs::remove_dir_all(dir);
    }

    pub fn read_verified_preview(&self, buf: &mut [u8], abs: i64) -> (usize, bool) {
        let mut inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.chunk_length <= 0 || buf.is_empty() {
            return (0, false);
        }
        let index = (abs / inner.chunk_length) as usize;
        if !inner.completed.get(&index).copied().unwrap_or(false) {
            return (0, false);
        }
        let chunk_len = inner.chunk_length;
        match self.read_piece_data_locked(&mut inner, index, abs % chunk_len, buf) {
            Ok(n) => (n, true),
            Err(_) => (0, false),
        }
    }

    // --- Write-back Cache & I/O

    fn ensure_cache_slot_locked(
        &self,
        inner: &mut StorageInner,
        index: usize,
        expected: i64,
    ) -> StorageResult<bool> {
        if let Some(existing) = inner.cache.get(&index) {
            if existing.len() as i64 == expected {
                inner.cache_order.touch(index as i32);
                return Ok(false);
            }
            self.drop_cache_locked(inner, index);
        }

        while inner.cache_bytes + expected > inner.max_cache_bytes {
            let mut victim = None;
            inner.cache_order.each(|p| {
                let p_idx = p as usize;
                if p_idx == index {
                    return true;
                }
                if inner.cache_dirty.get(&p_idx).copied().unwrap_or(false) {
                    return true;
                }
                victim = Some(p_idx);
                false
            });

            if let Some(v) = victim {
                self.drop_cache_locked(inner, v);
            } else {
                let mut dirty_victim = None;
                inner.cache_order.each(|p| {
                    let p_idx = p as usize;
                    if p_idx != index {
                        dirty_victim = Some(p_idx);
                        return false;
                    }
                    true
                });
                if let Some(dv) = dirty_victim {
                    self.flush_piece_locked(inner, dv)?;
                    self.drop_cache_locked(inner, dv);
                } else {
                    return Err(StorageError::Other("cache full of unflushable data".into()));
                }
            }
        }

        inner.cache.insert(index, vec![0u8; expected as usize]);
        inner.cache_bytes += expected;
        inner.cache_order.touch(index as i32);
        Ok(true)
    }

    /// Ensures a stream RAM slot. Returns `(created, pieces_to_refresh)` —
    /// callers must invoke `on_stream_evict` for those pieces **after**
    /// releasing the storage lock (Go `queueCompletionLocked` / drain).
    fn ensure_stream_slot_locked(
        &self,
        inner: &mut StorageInner,
        index: usize,
        expected: i64,
    ) -> StorageResult<(bool, Vec<usize>)> {
        let mut refresh = Vec::new();
        if let Some(data) = inner.cache.get(&index) {
            if data.len() as i64 == expected {
                inner.cache_order.touch(index as i32);
                return Ok((false, refresh));
            }
            self.drop_cache_locked(inner, index);
        }

        while inner.cache_bytes + expected > inner.max_cache_bytes {
            let mut victim = None;
            for pass in 0..2 {
                if victim.is_some() {
                    break;
                }
                inner.cache_order.each(|p| {
                    let p_idx = p as usize;
                    if p_idx == index {
                        return true;
                    }
                    let is_comp = inner.completed.get(&p_idx).copied().unwrap_or(false);
                    let is_whole = inner.stream_whole.get(&p_idx).copied().unwrap_or(false);
                    if !is_comp && is_whole {
                        return true;
                    }
                    if pass == 0 {
                        let is_demanded = inner.stream_demand.get(&p_idx).copied().unwrap_or(false);
                        let is_preload = inner
                            .preload_pieces
                            .as_ref()
                            .map(|x| x.contains_key(&p_idx))
                            .unwrap_or(false);
                        let is_startup = inner.startup_pieces.get(&p_idx).copied().unwrap_or(false);
                        let is_saved = inner.startup_saved.get(&p_idx).copied().unwrap_or(false);
                        if is_demanded
                            || (is_preload && !is_saved)
                            || (is_startup && is_comp && !is_saved)
                        {
                            return true;
                        }
                    }
                    victim = Some(p_idx);
                    false
                });
            }

            let v = match victim {
                Some(v) => v,
                None => return Err(StorageError::Other("stream cache busy".into())),
            };

            if !inner.completed.get(&v).copied().unwrap_or(false) {
                inner.stream_lost.insert(v, true);
            }
            self.drop_cache_locked(inner, v);
            inner.stream_ranges.remove(&v);
            inner.stream_whole.remove(&v);
            if !inner.startup_saved.get(&v).copied().unwrap_or(false) {
                inner.completed.remove(&v);
                refresh.push(v);
            }
        }

        inner.cache.insert(index, vec![0u8; expected as usize]);
        inner.cache_bytes += expected;
        inner.stream_revision += 1;
        inner.cache_order.touch(index as i32);
        Ok((true, refresh))
    }

    fn drop_cache_locked(&self, inner: &mut StorageInner, index: usize) {
        if let Some(data) = inner.cache.remove(&index) {
            inner.cache_bytes -= data.len() as i64;
            inner.cache_dirty.remove(&index);
            inner.cache_order.remove(index as i32);
        }
    }

    pub fn flush_piece_locked(&self, inner: &mut StorageInner, index: usize) -> StorageResult<()> {
        if !inner.cache_dirty.get(&index).copied().unwrap_or(false) {
            return Ok(());
        }
        let data = match inner.cache.get(&index) {
            Some(d) => d,
            None => return Ok(()),
        };
        let spans = inner.dirty_spans.remove(&index).unwrap_or_else(|| {
            vec![ByteRange {
                start: 0,
                end: data.len() as i64,
            }]
        });

        for span in spans {
            let start = (index as i64) * inner.chunk_length + span.start;
            let chunk = &data[span.start as usize..span.end as usize];
            if let Err(e) = Self::write_to_files_locked(&mut inner.files, start, chunk) {
                inner.flush_err = Some(e.to_string());
                return Err(e);
            }
        }

        inner.cache_dirty.insert(index, false);
        inner.flush_err = None;
        self.note_piece_files_need_sync_locked(inner, index);
        Ok(())
    }

    pub fn flush_all_dirty_locked(&self, inner: &mut StorageInner) -> StorageResult<()> {
        let dirty_indices: Vec<usize> = inner
            .cache_dirty
            .iter()
            .filter(|(_, &d)| d)
            .map(|(&idx, _)| idx)
            .collect();
        for idx in dirty_indices {
            self.flush_piece_locked(inner, idx)?;
        }
        Ok(())
    }

    fn note_piece_files_need_sync_locked(&self, inner: &mut StorageInner, index: usize) {
        let start = (index as i64) * inner.chunk_length;
        let end = start
            + inner
                .info
                .as_ref()
                .map(|i| i.piece(index as u32).length)
                .unwrap_or(0);
        for f in &inner.files {
            if f.offset + f.length <= start || f.offset >= end {
                continue;
            }
            if let Some(file) = &f.file {
                inner.pending_sync_fds.insert(file.as_raw_fd());
            }
        }
    }

    /// Locked convenience wrapper used by mark_verified / close.
    /// Prefer [`Self::drain_sync_and_checkpoint`] when test hooks may re-enter.
    fn drain_sync_and_checkpoint_locked(&self, _inner: &mut StorageInner) {
        // Drop the caller's guard by unlocking via parking_lot: we re-enter the
        // public drain which acquires its own guards. Caller must not use
        // `_inner` after this returns.
        //
        // Safety of unlock: parking_lot::MutexGuard is released by drop. We
        // cannot drop a &mut reference, so call sites that need hook-safe drain
        // should call `drain_sync_and_checkpoint` directly. For production
        // paths without hooks, inline a simple sync+write here.
        //
        // Keep a simple in-lock path for production (no hooks).
        self.drain_sync_and_checkpoint_locked_simple(_inner);
    }

    fn drain_sync_and_checkpoint_locked_simple(&self, inner: &mut StorageInner) {
        if inner.closed || !inner.attached {
            return;
        }
        let _ = self.flush_all_dirty_locked(inner);
        // When hooks are installed, bounce to the unlock-safe public path.
        if inner.file_sync_hook.is_some() || inner.write_checkpoint_hook.is_some() {
            // Can't safely unlock from &mut guard; production callers won't set hooks.
            // Tests must call `drain_sync_and_checkpoint()` instead.
        }
        for f in &inner.files {
            if let Some(file) = &f.file {
                if let Some(fd) = inner
                    .pending_sync_fds
                    .iter()
                    .find(|&&x| x == file.as_raw_fd())
                {
                    let _ = fd;
                    let _ = file.sync_all();
                }
            }
        }
        // Sync any pending fds' files.
        let pending: Vec<RawFd> = inner.pending_sync_fds.drain().collect();
        for fd in pending {
            for f in &inner.files {
                if let Some(file) = &f.file {
                    if file.as_raw_fd() == fd {
                        let _ = file.sync_all();
                    }
                }
            }
        }

        if !(inner.checkpoint_dirty && !inner.stream) {
            return;
        }
        let gen = inner.checkpoint_gen;
        self.capture_identities_locked(inner);
        let Some(info) = inner.info.as_ref() else {
            return;
        };
        let num_pieces = info.num_pieces() as usize;
        let mut complete = vec![false; num_pieces];
        for (&p, &ok) in &inner.completed {
            if p < num_pieces {
                complete[p] = ok;
            }
        }
        let cp = PieceCheckpoint {
            info_hash: self.info_hash.0,
            piece_length: inner.chunk_length,
            num_pieces,
            files: inner.identities.clone(),
            complete,
        };
        let path = checkpoint_path(&self.base_dir, &self.info_hash.to_hex());
        match write_piece_checkpoint(&path, &cp) {
            Ok(()) => {
                if inner.checkpoint_gen == gen {
                    inner.checkpoint_dirty = false;
                    inner.checkpoint_last_err = None;
                } else {
                    let _ = delete_piece_checkpoint(&self.base_dir, &self.info_hash.to_hex());
                }
            }
            Err(e) => {
                inner.checkpoint_last_err = Some(e.to_string());
            }
        }
    }

    fn capture_identities_locked(&self, inner: &mut StorageInner) {
        let mut ids = Vec::with_capacity(inner.files.len());
        for f in &inner.files {
            if let Some(file) = &f.file {
                let (id, _) = identity_from_file(file, f.external, f.length, f.path.clone());
                ids.push(id);
            } else {
                ids.push(FileIdentity {
                    external: f.external,
                    length: f.length,
                    path: f.path.clone(),
                    ..Default::default()
                });
            }
        }
        inner.identities = ids;
    }

    fn load_checkpoint_locked(
        &self,
        inner: &mut StorageInner,
        identities: &[FileIdentity],
    ) -> bool {
        let path = checkpoint_path(&self.base_dir, &self.info_hash.to_hex());
        let cp = match read_piece_checkpoint(&path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        if cp.info_hash != self.info_hash.0
            || cp.piece_length != inner.chunk_length
            || cp.num_pieces
                != inner
                    .info
                    .as_ref()
                    .map(|i| i.num_pieces() as usize)
                    .unwrap_or(0)
        {
            return false;
        }
        if cp.files.len() != identities.len() {
            return false;
        }

        for (p, &is_comp) in cp.complete.iter().enumerate() {
            if !is_comp {
                continue;
            }
            if self.piece_files_stable_locked(inner, p, &cp.files, identities) {
                inner.completed.insert(p, true);
            }
        }
        true
    }

    fn piece_files_stable_locked(
        &self,
        inner: &StorageInner,
        piece_idx: usize,
        saved: &[FileIdentity],
        current: &[FileIdentity],
    ) -> bool {
        let start = (piece_idx as i64) * inner.chunk_length;
        let end = start
            + inner
                .info
                .as_ref()
                .map(|i| i.piece(piece_idx as u32).length)
                .unwrap_or(0);
        for i in 0..inner.files.len() {
            let f = &inner.files[i];
            if f.offset + f.length <= start || f.offset >= end {
                continue;
            }
            if !file_stable(&saved[i], &current[i]) {
                return false;
            }
        }
        true
    }

    fn fill_partial_slot_locked(
        &self,
        inner: &mut StorageInner,
        index: usize,
        off: i64,
        n: i64,
    ) -> StorageResult<()> {
        let spans = match inner.partial_slots.get(&index) {
            Some(s) => s.clone(),
            None => return Ok(()),
        };
        if ranges_cover(&spans, off, off + n) {
            return Ok(());
        }

        self.flush_piece_locked(inner, index)?;

        let start = (index as i64) * inner.chunk_length;
        if let Some(buf) = inner.cache.get_mut(&index) {
            Self::read_from_files_locked(&inner.files, start, buf)?;
        }
        inner.partial_slots.remove(&index);
        Ok(())
    }

    fn read_piece_data_locked(
        &self,
        inner: &mut StorageInner,
        index: usize,
        off: i64,
        buf: &mut [u8],
    ) -> StorageResult<usize> {
        let piece_len = inner.info.as_ref().unwrap().piece(index as u32).length;
        if off < 0 || off >= piece_len {
            return Ok(0);
        }

        if inner.cache.contains_key(&index) {
            self.fill_partial_slot_locked(inner, index, off, buf.len() as i64)?;
            inner.cache_order.touch(index as i32);
            if let Some(data) = inner.cache.get(&index) {
                let n = (piece_len - off).min(buf.len() as i64) as usize;
                buf[..n].copy_from_slice(&data[off as usize..off as usize + n]);
                return Ok(n);
            }
        }

        if inner.stream {
            if inner.startup_saved.get(&index).copied().unwrap_or(false) {
                let path = self.startup_piece_path(index);
                let mut file = File::open(path)?;
                file.seek(SeekFrom::Start((32 + off) as u64))?;
                let n = (piece_len - off).min(buf.len() as i64) as usize;
                file.read_exact(&mut buf[..n])?;
                return Ok(n);
            }
            return Err(StorageError::Unavailable);
        }

        let abs = (index as i64) * inner.chunk_length + off;
        let n = (piece_len - off).min(buf.len() as i64) as usize;
        Self::read_from_files_locked(&inner.files, abs, &mut buf[..n])
    }

    pub fn write_to_files_locked(
        files: &mut [FileDescriptor],
        start: i64,
        data: &[u8],
    ) -> StorageResult<usize> {
        let end = start + data.len() as i64;
        let mut done = 0usize;
        for f in files.iter_mut() {
            if f.offset + f.length <= start || f.offset >= end {
                continue;
            }
            let from = start.max(f.offset);
            let to = end.min(f.offset + f.length);
            if from >= to {
                continue;
            }
            let file = f.file.as_mut().ok_or_else(|| {
                StorageError::Other(format!("storage for {} is not configured", f.path))
            })?;
            let chunk = &data[(from - start) as usize..(to - start) as usize];
            file.write_all_at(chunk, (from - f.offset) as u64)?;
            done += chunk.len();
        }
        if done != data.len() {
            return Err(StorageError::Other("short write to files".into()));
        }
        Ok(done)
    }

    pub fn read_from_files_locked(
        files: &[FileDescriptor],
        start: i64,
        buf: &mut [u8],
    ) -> StorageResult<usize> {
        let end = start + buf.len() as i64;
        let mut done = 0usize;
        for f in files {
            if f.offset + f.length <= start || f.offset >= end {
                continue;
            }
            let from = start.max(f.offset);
            let to = end.min(f.offset + f.length);
            if from >= to {
                continue;
            }
            let file = f.file.as_ref().ok_or_else(|| {
                StorageError::Other(format!("storage for {} is not configured", f.path))
            })?;
            let chunk = &mut buf[(from - start) as usize..(to - start) as usize];
            file.read_exact_at(chunk, (from - f.offset) as u64)?;
            done += chunk.len();
        }
        if done != buf.len() {
            return Err(StorageError::Other("short read from files".into()));
        }
        Ok(done)
    }

    fn save_startup_piece_locked(&self, inner: &mut StorageInner, index: usize) {
        if !inner.startup_pieces.get(&index).copied().unwrap_or(false)
            || inner.startup_saved.get(&index).copied().unwrap_or(false)
            || inner.startup_pending.contains_key(&index)
            || inner.startup_in_flight == Some(index)
        {
            return;
        }
        let Some(data) = inner.cache.get(&index) else {
            return;
        };
        let length = inner
            .info
            .as_ref()
            .map(|i| i.piece(index as u32).length)
            .unwrap_or(0);
        if data.len() as i64 != length
            || inner.startup_pending_bytes + data.len()
                > (DIRECT_STARTUP_BUDGET + DIRECT_PRELOAD_BUDGET) as usize
        {
            return;
        }
        inner.startup_pending_bytes += data.len();
        inner.startup_pending.insert(index, data.clone());
        if !inner.startup_writing {
            inner.startup_writing = true;
            let storage = self.weak_self.lock().upgrade().expect("storage dropped");
            std::thread::spawn(move || storage.drain_startup_pieces());
        }
    }

    fn drain_startup_pieces(&self) {
        loop {
            let (index, data) = {
                let mut inner = self.inner.lock();
                let Some(index) = inner.startup_pending.keys().next().copied() else {
                    inner.startup_writing = false;
                    self.startup_drained.notify_all();
                    return;
                };
                inner.startup_in_flight = Some(index);
                (index, inner.startup_pending.remove(&index).unwrap())
            };
            #[cfg(test)]
            if let Some(hook) = self.startup_save_hook.lock().clone() {
                hook();
            }
            let path = self.startup_piece_path(index);
            let tmp = path.with_extension("tmp");
            let written = (|| -> std::io::Result<()> {
                let sum = Sha256::digest(&data);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut f = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&tmp)?;
                f.write_all(&sum)?;
                f.write_all(&data)?;
                f.sync_all()?;
                Ok(())
            })();
            let mut inner = self.inner.lock();
            inner.startup_pending_bytes -= data.len();
            inner.startup_in_flight = None;
            let committed = written.is_ok()
                && !inner.closed
                && inner.startup_pieces.get(&index).copied().unwrap_or(false)
                && fs::rename(&tmp, &path).is_ok();
            if committed {
                if inner.cache.get(&index) != Some(&data) {
                    self.drop_cache_locked(&mut inner, index);
                    inner.stream_ranges.remove(&index);
                    inner.stream_whole.remove(&index);
                }
                inner.stream_lost.remove(&index);
                inner.startup_saved.insert(index, true);
                inner.completed.insert(index, true);
                inner.stream_ready.notify();
                // Retry verified startup pieces skipped while the bounded queue was full.
                let candidates: Vec<usize> = inner
                    .startup_pieces
                    .keys()
                    .copied()
                    .filter(|p| inner.completed.get(p).copied().unwrap_or(false))
                    .collect();
                for p in candidates {
                    self.save_startup_piece_locked(&mut inner, p);
                }
            } else {
                let _ = fs::remove_file(&tmp);
            }
        }
    }
}

impl TorrentStorage for DocumentTorrentStorage {
    fn piece(&self, piece: Piece, _hash: Option<[u8; 20]>) -> Arc<dyn PieceStorage> {
        let arc = self.weak_self.lock().upgrade().expect("storage dropped");
        Arc::new(DocumentPiece {
            storage: arc,
            piece,
        })
    }

    fn close(&self) -> StorageResult<()> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Ok(());
        }
        while inner.startup_writing {
            self.startup_drained.wait(&mut inner);
        }
        if inner.closed {
            return Ok(());
        }
        let mut flush_res = Ok(());
        if inner.attached && !inner.stream {
            flush_res = self.flush_all_dirty_locked(&mut inner);
            self.drain_sync_and_checkpoint_locked(&mut inner);
        }
        inner.closed = true;
        inner.checkpoint_gen = inner.checkpoint_gen.wrapping_add(1);
        inner.stream_ready.notify();
        if inner.stream {
            crate::direct_storage::unregister_startup_cache(
                &self.base_dir,
                &self.info_hash.to_hex(),
            );
        }
        for f in &mut inner.files {
            let _ = f.file.take();
        }
        inner.attached = false;
        inner.cache.clear();
        inner.cache_dirty.clear();
        inner.completed.clear();
        inner.cache_bytes = 0;
        inner.dirty_spans.clear();
        inner.partial_slots.clear();
        inner.saved_partial.clear();
        inner.cache_order.reset();
        inner.stream_revision += 1;
        inner.stream_grid_runs.clear();
        // Direct Play: do not flush when destination files were never configured.
        // (flush_res already skipped above when `inner.stream`).
        flush_res
    }
}

// ---------------------------------------------------------------- DocumentPiece

pub struct DocumentPiece {
    pub storage: Arc<DocumentTorrentStorage>,
    pub piece: Piece,
}

impl DocumentPiece {
    pub fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize> {
        <Self as PieceStorage>::read_at(self, buf, off)
    }

    pub fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize> {
        <Self as PieceStorage>::write_at(self, buf, off)
    }

    pub fn mark_complete(&self) -> StorageResult<()> {
        <Self as PieceStorage>::mark_complete(self)
    }

    pub fn mark_not_complete(&self) -> StorageResult<()> {
        <Self as PieceStorage>::mark_not_complete(self)
    }

    pub fn completion(&self) -> Completion {
        <Self as PieceStorage>::completion(self)
    }

    pub fn piece_extent_all_holes(&self) -> (bool, bool) {
        self.storage.piece_extent_all_holes(self.piece)
    }

    pub fn self_hash(&self) -> StorageResult<[u8; 20]> {
        let index = self.piece.index as usize;
        let length = self.piece.length;

        // In RAM?
        let in_ram = {
            let inner = self.storage.inner.lock();
            inner.cache.contains_key(&index)
        };
        if in_ram {
            return self.hash_contents();
        }

        // File spans
        if let Some(spans) = self.storage.piece_spans(self.piece) {
            if let Ok((sum, all_holes)) = hash_extents(&spans) {
                if all_holes {
                    return Ok(zero_piece_hash(length));
                }
                return Ok(sum);
            }
        }

        // Sparse holes probe
        let (all_holes, supported) = self.storage.piece_extent_all_holes(self.piece);
        if supported && all_holes {
            return Ok(zero_piece_hash(length));
        }

        self.hash_contents()
    }

    fn hash_contents(&self) -> StorageResult<[u8; 20]> {
        let mut hasher = Sha1::new();
        let mut buf = [0u8; 64 * 1024];
        let mut off = 0i64;
        let length = self.piece.length;

        while off < length {
            let want = ((length - off) as usize).min(buf.len());
            let n = self.read_at(&mut buf[..want], off)?;
            if n == 0 {
                return Err(StorageError::Other("unexpected eof in piece read".into()));
            }
            hasher.update(&buf[..n]);
            off += n as i64;
        }
        let mut out = [0u8; 20];
        out.copy_from_slice(&hasher.finalize());
        Ok(out)
    }
}

impl PieceStorage for DocumentPiece {
    fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize> {
        let mut inner = self.storage.inner.lock();
        if inner.closed {
            return Err(StorageError::Closed);
        }
        let index = self.piece.index as usize;
        if inner.stream_lost.get(&index).copied().unwrap_or(false) {
            return Err(StorageError::Unavailable);
        }

        if inner.cache.contains_key(&index) {
            self.storage
                .fill_partial_slot_locked(&mut inner, index, off, buf.len() as i64)?;
            inner.cache_order.touch(index as i32);
            if let Some(data) = inner.cache.get(&index) {
                if off >= data.len() as i64 {
                    return Ok(0);
                }
                let n = ((data.len() as i64 - off) as usize).min(buf.len());
                buf[..n].copy_from_slice(&data[off as usize..off as usize + n]);
                return Ok(n);
            }
        }

        if !inner.attached {
            return Err(StorageError::Other(
                "download storage is not configured".into(),
            ));
        }

        if inner.stream {
            if !inner.startup_saved.get(&index).copied().unwrap_or(false) {
                return Err(StorageError::Unavailable);
            }
            let path = self.storage.startup_piece_path(index);
            let mut file = File::open(path)?;
            if off < 0 || off >= self.piece.length {
                return Ok(0);
            }
            file.seek(SeekFrom::Start((32 + off) as u64))?;
            let n = ((self.piece.length - off) as usize).min(buf.len());
            file.read_exact(&mut buf[..n])?;
            return Ok(n);
        }

        let abs = (index as i64) * inner.chunk_length + off;
        let length = self.piece.length;
        if off < 0 || off >= length {
            return Ok(0);
        }
        let n = ((length - off) as usize).min(buf.len());
        DocumentTorrentStorage::read_from_files_locked(&inner.files, abs, &mut buf[..n])
    }

    fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize> {
        let mut inner = self.storage.inner.lock();
        if inner.closed {
            return Err(StorageError::Closed);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let index = self.piece.index as usize;
        let expected = self.piece.length;
        if off < 0 || off + buf.len() as i64 > expected {
            return Err(StorageError::Other("invalid piece write bounds".into()));
        }
        if inner.stream && inner.completed.get(&index).copied().unwrap_or(false) {
            return Ok(buf.len());
        }

        if !inner.attached {
            if !inner.cache.contains_key(&index) {
                if inner.cache_bytes + expected > inner.max_cache_bytes {
                    return Err(StorageError::Other("prepared data cache is full".into()));
                }
                inner.cache.insert(index, vec![0u8; expected as usize]);
                inner.cache_bytes += expected;
                inner.cache_order.touch(index as i32);
            }
            let data = inner.cache.get_mut(&index).unwrap();
            data[off as usize..off as usize + buf.len()].copy_from_slice(buf);
            inner.cache_dirty.insert(index, true);
            let spans = inner.dirty_spans.entry(index).or_default();
            merge_received_range(
                spans,
                ByteRange {
                    start: off,
                    end: off + buf.len() as i64,
                },
            );
            return Ok(buf.len());
        }

        // Attached:
        let mut pending_refresh = Vec::new();
        let created = if inner.stream {
            let (created, refresh) = self
                .storage
                .ensure_stream_slot_locked(&mut inner, index, expected)?;
            pending_refresh = refresh;
            created
        } else {
            match self
                .storage
                .ensure_cache_slot_locked(&mut inner, index, expected)
            {
                Ok(c) => c,
                Err(_) => {
                    // Cache full of dirty: fallback to disk write
                    let start = (index as i64) * inner.chunk_length + off;
                    return DocumentTorrentStorage::write_to_files_locked(
                        &mut inner.files,
                        start,
                        buf,
                    );
                }
            }
        };

        if created {
            if inner.stream
                || inner.completed.get(&index).copied().unwrap_or(false)
                || inner.saved_partial.get(&index).copied().unwrap_or(false)
            {
                // load from disk if partial
                if !inner.stream {
                    let start = (index as i64) * inner.chunk_length;
                    let p_len = inner
                        .info
                        .as_ref()
                        .map(|i| i.piece(index as u32).length)
                        .unwrap_or(0);
                    let mut data = vec![0u8; p_len as usize];
                    if DocumentTorrentStorage::read_from_files_locked(
                        &inner.files,
                        start,
                        &mut data,
                    )
                    .is_ok()
                    {
                        inner.cache.insert(index, data);
                    }
                }
            } else {
                inner.partial_slots.insert(index, Vec::new());
            }
        }

        if let Some(data) = inner.cache.get_mut(&index) {
            data[off as usize..off as usize + buf.len()].copy_from_slice(buf);
        }

        if let Some(written) = inner.partial_slots.get_mut(&index) {
            merge_received_range(
                written,
                ByteRange {
                    start: off,
                    end: off + buf.len() as i64,
                },
            );
            if written.len() == 1 && written[0].start == 0 && written[0].end == expected {
                inner.partial_slots.remove(&index);
            }
        }

        if inner.stream {
            let is_lost = inner.stream_lost.get(&index).copied().unwrap_or(false);
            let merged = inner.stream_ranges.entry(index).or_default();
            merge_received_range(
                merged,
                ByteRange {
                    start: off,
                    end: off + buf.len() as i64,
                },
            );
            let whole =
                !is_lost && merged.len() == 1 && merged[0].start == 0 && merged[0].end == expected;
            inner.stream_whole.insert(index, whole);
            if inner.stream_watch.get(&index).copied().unwrap_or(false) {
                inner.stream_ready.notify();
            }
        }

        inner.cache_dirty.insert(index, true);
        let spans = inner.dirty_spans.entry(index).or_default();
        merge_received_range(
            spans,
            ByteRange {
                start: off,
                end: off + buf.len() as i64,
            },
        );
        inner.cache_order.touch(index as i32);

        let callback = inner.on_stream_evict.clone();
        drop(inner);
        if let Some(cb) = callback {
            for v in pending_refresh {
                let cb = Arc::clone(&cb);
                std::thread::spawn(move || cb(v));
            }
        }

        Ok(buf.len())
    }

    fn mark_complete(&self) -> StorageResult<()> {
        let mut inner = self.storage.inner.lock();
        if inner.closed {
            return Err(StorageError::Closed);
        }
        let index = self.piece.index as usize;
        if inner.stream {
            let is_lost = inner.stream_lost.get(&index).copied().unwrap_or(false);
            let is_whole = inner.stream_whole.get(&index).copied().unwrap_or(false);
            let is_saved = inner.startup_saved.get(&index).copied().unwrap_or(false);
            if is_lost || (!is_whole && !is_saved) {
                return Err(StorageError::Unavailable);
            }
            inner.completed.insert(index, true);
            inner.stream_revision += 1;
            self.storage.save_startup_piece_locked(&mut inner, index);
            inner.stream_ready.notify();
            // Fire on_stream_evict asynchronously (Go drainCompletionUpdates).
            // Close must not join a blocked callback.
            let callback = inner.on_stream_evict.clone();
            drop(inner);
            if let Some(cb) = callback {
                std::thread::spawn(move || cb(index));
            }
            return Ok(());
        }

        // Download mode: flush piece to disk
        self.storage.flush_piece_locked(&mut inner, index)?;
        inner.completed.insert(index, true);
        inner.saved_partial.remove(&index);
        self.storage
            .note_piece_files_need_sync_locked(&mut inner, index);
        inner.checkpoint_dirty = true;
        Ok(())
    }

    fn mark_not_complete(&self) -> StorageResult<()> {
        let mut inner = self.storage.inner.lock();
        if inner.closed || !inner.attached {
            return Ok(());
        }
        let index = self.piece.index as usize;
        if self.storage.freeze_trusted.load(Ordering::SeqCst) {
            return Ok(());
        }
        if inner.completed.get(&index).copied().unwrap_or(false) {
            return Ok(());
        }
        inner.completed.remove(&index);
        if inner.stream {
            inner.stream_lost.remove(&index);
            inner.stream_ranges.remove(&index);
            inner.stream_whole.remove(&index);
        }
        self.storage.drop_cache_locked(&mut inner, index);
        inner.checkpoint_dirty = true;
        Ok(())
    }

    fn completion(&self) -> Completion {
        let inner = self.storage.inner.lock();
        if inner.closed {
            return Completion {
                ok: false,
                complete: false,
                err: Some("storage closed".into()),
            };
        }
        if !inner.attached {
            return Completion {
                ok: false,
                complete: false,
                err: None,
            };
        }
        let is_comp = inner
            .completed
            .get(&(self.piece.index as usize))
            .copied()
            .unwrap_or(false);
        Completion {
            ok: true,
            complete: is_comp,
            err: None,
        }
    }
}

// ---------------------------------------------------------------- Helpers

/// Files holding bytes of `[start, end)` (Go `filesOverlapping`).
/// Files are in torrent order; binary search finds the overlapping slice.
pub fn files_overlapping(files: &[FileDescriptor], start: i64, end: i64) -> &[FileDescriptor] {
    let first = files.partition_point(|f| f.offset + f.length <= start);
    let last = first + files[first..].partition_point(|f| f.offset < end);
    &files[first..last]
}

pub fn startup_file_pieces(f: &FileDescriptor, chunk: i64) -> Vec<usize> {
    if chunk <= 0 {
        return Vec::new();
    }
    let mut pieces = Vec::new();
    let spans = [
        (f.offset, f.offset + f.length.min(DIRECT_STARTUP_HEAD)),
        (
            f.offset + 0i64.max(f.length - DIRECT_STARTUP_TAIL),
            f.offset + f.length,
        ),
    ];
    for (start, end) in spans {
        if end <= start {
            continue;
        }
        let begin_p = (start / chunk) as usize;
        let last_p = ((end - 1) / chunk) as usize;
        for p in begin_p..=last_p {
            if !pieces.contains(&p) {
                pieces.push(p);
            }
        }
    }
    pieces
}

pub fn is_video_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = match lower.rfind('.') {
        Some(pos) => &lower[pos..],
        None => return false,
    };
    matches!(
        ext,
        ".mp4"
            | ".m4v"
            | ".mkv"
            | ".webm"
            | ".avi"
            | ".mov"
            | ".wmv"
            | ".flv"
            | ".ts"
            | ".m2ts"
            | ".mts"
            | ".vob"
            | ".f4v"
            | ".mpg"
            | ".mpeg"
            | ".m2v"
            | ".3gp"
            | ".ogv"
            | ".iso"
    )
}

pub fn engine_owned_part_path(
    base_dir: &Path,
    info_hash_hex: &str,
    file_index: usize,
    stream_selected: bool,
) -> PathBuf {
    let root = if stream_selected {
        base_dir.join(".stream").join(info_hash_hex)
    } else {
        base_dir.join(".boundary").join(info_hash_hex)
    };
    root.join(format!("{}.part", file_index))
}

pub fn delete_engine_owned_storage(base_dir: &Path, info_hash_hex: &str) -> StorageResult<()> {
    for dir in &[
        base_dir.join(".stream").join(info_hash_hex),
        base_dir.join(".boundary").join(info_hash_hex),
        base_dir.join(".startup").join(info_hash_hex),
        base_dir.join(".preview").join(info_hash_hex),
    ] {
        let _ = fs::remove_dir_all(dir);
    }
    Ok(())
}

#[cfg(test)]
mod startup_save_tests {
    use super::*;
    use std::sync::mpsc;
    use wt_types::FileInfo;

    #[test]
    fn piece_grid_states_match_go_storage_precedence() {
        let storage = DocumentTorrentStorage::new(None, InfoHash([9; 20]), Path::new("."));
        {
            let mut inner = storage.inner.lock();
            for index in [0, 1, 2, 3, 4, 5] {
                inner.cache.insert(index, vec![1]);
                inner
                    .stream_ranges
                    .insert(index, vec![ByteRange { start: 0, end: 1 }]);
            }
            for index in [0, 1, 4, 7] {
                inner.completed.insert(index, true);
            }
            for index in [1, 2, 4, 6] {
                inner.startup_saved.insert(index, true);
            }
            for index in [4, 5] {
                inner.stream_lost.insert(index, true);
            }
            inner.stream_revision = 42;
        }
        assert_eq!(storage.piece_grid_states(0, 8), b"vvsrsmsm");
        assert_eq!(storage.piece_grid_states(2, 5), b"srs");
        assert!(storage.piece_grid_states(5, 2).is_empty());
        assert_eq!(storage.stream_revision(), 42);
    }

    #[test]
    fn slow_startup_save_does_not_block_piece_traffic() {
        let base = PathBuf::from("/home/kar/.cache/wt-tmp/perf");
        fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("startup-save-")
            .tempdir_in(base)
            .unwrap();
        let info = Info {
            name: "movie.mp4".into(),
            piece_length: 16,
            pieces: vec![0; 80],
            files: vec![FileInfo {
                length: 64,
                path: vec![],
                torrent_offset: 0,
                attr: String::new(),
                pieces_root: None,
            }],
            single_file: true,
            private: None,
            source: String::new(),
            meta_version: 0,
            file_tree_raw: None,
        };
        let storage = DocumentTorrentStorage::new(Some(info), InfoHash([7; 20]), dir.path());
        storage.attach_direct_play(&[None], &[0]).unwrap();
        storage.inner.lock().startup_pieces.insert(0, true);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        *storage.startup_save_hook.lock() = Some(Arc::new(move || {
            started_tx.send(()).unwrap();
            release_rx.lock().recv().unwrap();
        }));
        let piece = storage.piece_at(0);
        piece.write_at(&[1; 16], 0).unwrap();
        piece.mark_complete().unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let other = storage.piece_at(1);
        let thread = std::thread::spawn(move || {
            done_tx.send(other.write_at(&[2; 16], 0)).unwrap();
        });
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        thread.join().unwrap();
        assert!(
            result.unwrap().is_ok(),
            "piece write blocked by startup save"
        );
        storage.close().unwrap();
    }

    #[test]
    fn slow_startup_save_does_not_block_readers() {
        let base = PathBuf::from("/home/kar/.cache/wt-tmp/perf");
        fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("startup-save-readers-")
            .tempdir_in(base)
            .unwrap();
        let info = Info {
            name: "movie.mp4".into(),
            piece_length: 16,
            pieces: vec![0; 80],
            files: vec![FileInfo {
                length: 64,
                path: vec![],
                torrent_offset: 0,
                attr: String::new(),
                pieces_root: None,
            }],
            single_file: true,
            private: None,
            source: String::new(),
            meta_version: 0,
            file_tree_raw: None,
        };
        let storage = DocumentTorrentStorage::new(Some(info), InfoHash([7; 20]), dir.path());
        storage.attach_direct_play(&[None], &[0]).unwrap();
        storage.inner.lock().startup_pieces.insert(0, true);

        // Populate piece 1 in cache before startup save starts.
        let other = storage.piece_at(1);
        other.write_at(&[42; 16], 0).unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        *storage.startup_save_hook.lock() = Some(Arc::new(move || {
            started_tx.send(()).unwrap();
            release_rx.lock().recv().unwrap();
        }));

        let piece = storage.piece_at(0);
        piece.write_at(&[1; 16], 0).unwrap();
        piece.mark_complete().unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // While startup save of piece 0 is blocked in I/O hook, read from piece 1 in RAM.
        let (done_tx, done_rx) = mpsc::channel();
        let reader_other = storage.piece_at(1);
        let reader_piece0 = storage.piece_at(0);
        let thread = std::thread::spawn(move || {
            let mut buf1 = [0u8; 16];
            let n1 = reader_other.read_at(&mut buf1, 0).expect("read piece 1");
            let mut buf0 = [0u8; 16];
            let n0 = reader_piece0.read_at(&mut buf0, 0).expect("read piece 0");
            done_tx.send((n1, buf1, n0, buf0)).unwrap();
        });
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        thread.join().unwrap();
        let (n1, buf1, n0, buf0) = result.expect("read timed out: reader blocked by startup save");
        assert_eq!(n1, 16);
        assert_eq!(buf1, [42; 16]);
        assert_eq!(n0, 16);
        assert_eq!(buf0, [1; 16]);
        storage.close().unwrap();
    }
}

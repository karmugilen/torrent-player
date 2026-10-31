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
use std::io::{Read, Write};
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
    /// Shared so unlocked I/O needs no descriptor duplicated under the lock.
    pub file: Option<Arc<File>>,
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
    /// Signalled when an unlocked piece flush finishes.
    flush_done: Condvar,
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
    /// Shared so a piece being hashed or written is not copied under the lock.
    /// Stream slots are never shared. A write to a shared slot copies it first.
    pub cache: wt_types::IndexMap<usize, Arc<Vec<u8>>>,
    pub cache_dirty: wt_types::IndexMap<usize, bool>,
    pub dirty_spans: wt_types::IndexMap<usize, Vec<ByteRange>>,
    pub partial_slots: wt_types::IndexMap<usize, Vec<ByteRange>>,
    pub saved_partial: wt_types::IndexMap<usize, bool>,
    pub cache_order: PieceLRU,
    pub cache_bytes: i64,
    pub max_cache_bytes: i64,
    pub download_max_bytes: i64,
    pub stream_max_bytes: i64,

    // Completion & checkpoints
    pub completed: wt_types::IndexMap<usize, bool>,
    pub identities: Vec<FileIdentity>,
    pub checkpoint_dirty: bool,
    pub checkpoint_last_err: Option<String>,
    pub flush_err: Option<String>,
    pub pending_sync_fds: HashSet<RawFd>,

    // Direct play
    startup_pending: wt_types::IndexMap<usize, Vec<u8>>,
    startup_in_flight: Option<usize>,
    startup_pending_bytes: usize,
    startup_writing: bool,
    pub startup_pieces: wt_types::IndexMap<usize, bool>,
    pub startup_saved: wt_types::IndexMap<usize, bool>,
    pub startup_on_disk: wt_types::IndexMap<usize, bool>,
    pub startup_order: Vec<usize>,
    pub startup_file: usize,
    pub startup_planned: bool,
    pub direct_selected: Vec<usize>,
    pub preload_pieces: Option<HashMap<usize, bool>>,
    pub preload_file: usize,

    pub stream_demand: HashMap<usize, bool>,
    pub stream_lost: wt_types::IndexMap<usize, bool>,
    pub stream_whole: wt_types::IndexMap<usize, bool>,
    pub stream_ranges: wt_types::IndexMap<usize, Vec<ByteRange>>,
    pub stream_revision: u64,
    pub stream_grid_runs: Vec<PieceRun>,
    pub stream_grid_revision: u64,
    pub stream_grid_total: usize,

    pub stream_watch: wt_types::IndexMap<usize, bool>,
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

    /// Bumped whenever file handles are replaced or released. Unlocked I/O
    /// that started on older handles retries against the new handles.
    pub files_gen: u64,
    /// Pieces whose dirty bytes are being written without the lock. They
    /// keep their cache slot until that write is published.
    pub flushing: wt_types::IndexSet<usize>,
    /// Pieces chosen as cache victims. Receives wait instead of landing in
    /// the slot, so the eviction flush can finish clean and free RAM.
    /// Without this, every active piece stays dirty, the cache never turns
    /// over, and new blocks write through 16 KiB at a time.
    pub evicting: wt_types::IndexSet<usize>,
    /// Flushing pieces written meanwhile; their snapshot may be stale.
    pub flush_raced: wt_types::IndexSet<usize>,
    /// Ranges written to a piece while it was being written back. Only these
    /// stay dirty after the write-back, not everything received before it.
    pub flush_new: wt_types::IndexMap<usize, Vec<ByteRange>>,
    /// Go `checkpointFlushing` / `checkpointFlushScheduled`.
    pub checkpoint_flushing: bool,
    pub checkpoint_flush_scheduled: bool,
    /// Go `checkpointRetry` backoff after a failed sync/checkpoint write.
    pub checkpoint_retry: Duration,
    pub checkpoint_failures: u32,
    /// Bumped with each completion change, so a publish that raced one
    /// leaves the checkpoint dirty.
    pub checkpoint_seq: u64,
    /// Test seam: keeps fixtures from depending on a write-back pass.
    #[cfg(test)]
    pub disk_flushing: bool,
}

/// Reads this large leave the storage lock before copying.
const LARGE_READ_BYTES: usize = 1 << 20;

/// How often saved data is synced and completion published. Each fsync holds
/// up writes to the file, and written data already survives an app kill. Only
/// power loss needs the sync; a checkpoint is written after its sync, so that
/// just repeats the work since the last one.
pub const CHECKPOINT_FLUSH_DELAY: Duration = Duration::from_secs(30);

/// RAM a saved download keeps pieces in until they verify. It only fills as far
/// as pieces are unfinished, since a verified piece leaves RAM once written.
fn saved_download_cache_bytes(setting: i64, piece_length: i64) -> i64 {
    setting
        .max(256 << 20)
        .max(piece_length.saturating_mul(8).min(512 << 20))
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

                cache: Default::default(),
                cache_dirty: Default::default(),
                dirty_spans: Default::default(),
                partial_slots: Default::default(),
                saved_partial: Default::default(),
                cache_order: PieceLRU::new(),
                cache_bytes: 0,
                max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
                download_max_bytes: DEFAULT_MAX_CACHE_BYTES,
                stream_max_bytes: DEFAULT_STREAM_CACHE_MAX_BYTES,

                completed: Default::default(),
                identities: Vec::new(),
                checkpoint_dirty: false,
                checkpoint_last_err: None,
                flush_err: None,
                pending_sync_fds: HashSet::new(),

                startup_pending: Default::default(),
                startup_in_flight: None,
                startup_pending_bytes: 0,
                startup_writing: false,
                startup_pieces: Default::default(),
                startup_saved: Default::default(),
                startup_on_disk: Default::default(),
                startup_order: Vec::new(),
                startup_file: 0,
                startup_planned: false,
                direct_selected: Vec::new(),
                preload_pieces: None,
                preload_file: 0,

                stream_demand: HashMap::new(),
                stream_lost: Default::default(),
                stream_whole: Default::default(),
                stream_ranges: Default::default(),
                stream_revision: 0,
                stream_grid_runs: Vec::new(),
                stream_grid_revision: 0,
                stream_grid_total: 0,

                stream_watch: Default::default(),
                stream_ready,

                on_stream_evict: None,
                checkpoint_gen: 0,
                flush_retry: Duration::from_millis(0),
                file_sync_hook: None,
                write_checkpoint_hook: None,
                files_gen: 0,
                flushing: Default::default(),
                evicting: Default::default(),
                flush_raced: Default::default(),
                flush_new: Default::default(),
                checkpoint_flushing: false,
                checkpoint_flush_scheduled: false,
                checkpoint_retry: Duration::from_millis(0),
                checkpoint_failures: 0,
                checkpoint_seq: 0,
                #[cfg(test)]
                disk_flushing: false,
            }),
            info_hash,
            base_dir: base_dir.as_ref().to_path_buf(),
            restore_verify: AtomicBool::new(false),
            freeze_trusted: AtomicBool::new(false),
            weak_self: Mutex::new(Weak::new()),
            startup_drained: Condvar::new(),
            flush_done: Condvar::new(),
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
        let _ = self.drain_sync_and_checkpoint_with(true);
    }

    /// `flush_dirty` first writes every cached dirty piece (explicit drains).
    /// Scheduled drains, like Go's, only make completed pieces durable: the
    /// write-back cache keeps partial pieces. Returns false when a sync or
    /// checkpoint write failed and must be retried.
    fn drain_sync_and_checkpoint_with(&self, flush_dirty: bool) -> bool {
        // Phase 1: snapshot under lock.
        let (gen, seq, checkpoint, syncs, sync_hook, write_hook, hash_hex, base) = {
            let mut inner = self.inner.lock();
            if inner.closed || !inner.attached {
                return true;
            }
            if flush_dirty {
                let _ = self.flush_all_dirty_locked(&mut inner);
            }
            // Only completion bits present before taking pending syncs may
            // become durable in this pass. Later completions need their own
            // data sync, even when they share the same destination file.
            let checkpoint = if inner.checkpoint_dirty && !inner.stream {
                if let Some(info) = inner.info.as_ref() {
                    let num_pieces = info.num_pieces() as usize;
                    let mut complete = vec![false; num_pieces];
                    for (&index, &ok) in &inner.completed {
                        if index < num_pieces {
                            complete[index] = ok;
                        }
                    }
                    let files = match inner
                        .files
                        .iter()
                        .map(FileIdentitySnapshot::new)
                        .collect::<std::io::Result<Vec<_>>>()
                    {
                        Ok(files) => files,
                        Err(e) => {
                            inner.checkpoint_last_err = Some(e.to_string());
                            return false;
                        }
                    };
                    Some((
                        PieceCheckpoint {
                            info_hash: self.info_hash.0,
                            piece_length: inner.chunk_length,
                            num_pieces,
                            files: Vec::new(),
                            complete,
                        },
                        inner.files_gen,
                        files,
                    ))
                } else {
                    None
                }
            } else {
                None
            };
            let pending: Vec<RawFd> = inner.pending_sync_fds.drain().collect();
            // Duplicate the descriptors so fsync runs unlocked and stays
            // valid if storage closes or replaces a file meanwhile.
            let syncs: Vec<(RawFd, std::io::Result<Option<File>>)> = pending
                .into_iter()
                .map(|fd| {
                    let file = inner
                        .files
                        .iter()
                        .filter_map(|f| f.file.as_ref())
                        .find(|f| f.as_raw_fd() == fd);
                    (fd, file.map(|f| f.try_clone()).transpose())
                })
                .collect();
            (
                inner.checkpoint_gen,
                inner.checkpoint_seq,
                checkpoint,
                syncs,
                inner.file_sync_hook.clone(),
                inner.write_checkpoint_hook.clone(),
                self.info_hash.to_hex(),
                self.base_dir.clone(),
            )
        };

        // Phase 2: sync without holding the storage lock.
        let sync_started = std::time::Instant::now();
        let mut sync_err: Option<String> = None;
        let mut failed_fds: Vec<RawFd> = Vec::new();
        for (fd, file) in syncs {
            let result = if let Some(hook) = &sync_hook {
                hook(fd)
            } else {
                match file {
                    Ok(Some(file)) => file.sync_all(),
                    // No longer open in this storage: nothing left to sync.
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = result {
                sync_err = Some(e.to_string());
                failed_fds.push(fd);
            }
        }

        wt_types::probe::add(wt_types::probe::SYNCS, 1);
        wt_types::probe::add(wt_types::probe::SYNC_MS, sync_started.elapsed().as_millis() as u64);
        if sync_started.elapsed() >= Duration::from_millis(100) {
            wt_types::events::record("disk_sync", format!("{}ms", sync_started.elapsed().as_millis()));
        }
        if let Some(err) = sync_err {
            let mut inner = self.inner.lock();
            for fd in failed_fds {
                inner.pending_sync_fds.insert(fd);
            }
            inner.checkpoint_last_err = Some(err);
            return false;
        }

        let Some((mut cp, files_gen, identity_files)) = checkpoint else {
            return true;
        };

        // Phase 3: fstat may wait on MediaStore/FUSE, so sample metadata
        // unlocked using the descriptor snapshot taken before the data sync.
        cp.files = identity_files
            .into_iter()
            .map(FileIdentitySnapshot::capture)
            .collect();
        {
            let mut inner = self.inner.lock();
            if inner.closed || !inner.attached || inner.checkpoint_gen != gen {
                return true;
            }
            if inner.files_gen != files_gen {
                // Keep the dirty checkpoint for a fresh descriptor/sync pass.
                return false;
            }
            inner.identities = cp.files.clone();
        }
        let publish_gen = gen;
        let path = checkpoint_path(&base, &hash_hex);

        // Phase 4: persist without holding the lock (hooks may delete/bump gen).
        let write_result = if let Some(hook) = &write_hook {
            hook(&path, &cp)
        } else {
            write_piece_checkpoint(&path, &cp)
        };

        // Phase 5: finalize.
        let mut inner = self.inner.lock();
        if inner.checkpoint_gen != publish_gen || inner.files_gen != files_gen {
            let _ = delete_piece_checkpoint(&base, &hash_hex);
            inner.checkpoint_last_err = Some(
                write_result
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "checkpoint publish aborted: generation changed".into()),
            );
            return true;
        }
        match write_result {
            Ok(()) => {
                // A completion published meanwhile is not in this snapshot.
                if inner.checkpoint_seq == seq {
                    inner.checkpoint_dirty = false;
                }
                inner.checkpoint_last_err = None;
                true
            }
            Err(e) => {
                inner.checkpoint_last_err = Some(e.to_string());
                false
            }
        }
    }

    /// One pass over all unsaved data. Returns whether more remains. Saved
    /// downloads write a piece when it verifies (`mark_complete`), not on a timer.
    #[cfg(test)]
    fn run_disk_flush(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.stream {
            inner.disk_flushing = false;
            return false;
        }
        // Go flushAllDirtyLocked: every pass writes all dirty spans, so the
        // cache stays clean and eviction never waits on disk. Holding
        // receiving pieces back until complete filled the cache with dirty
        // slots under load, and evictions fell back to lock-held disk I/O.
        let dirty: Vec<usize> = inner
            .cache_dirty
            .iter()
            .filter(|&(i, &d)| d && !inner.flushing.contains(i))
            .map(|(&i, _)| i)
            .collect();
        let mut failed = None;
        for index in dirty {
            let (guard, result) = self.flush_piece_unlocked(inner, index);
            inner = guard;
            if inner.closed {
                inner.disk_flushing = false;
                return false;
            }
            if let Err(e) = result {
                failed = Some(e.to_string());
            }
        }
        match failed {
            Some(err) => {
                inner.flush_err = Some(err);
                inner.flush_retry = (inner.flush_retry * 2)
                    .max(Duration::from_millis(100))
                    .min(Duration::from_secs(5));
            }
            None => {
                inner.flush_err = None;
                inner.flush_retry = Duration::ZERO;
            }
        }
        let more = inner.cache_dirty.values().any(|&d| d);
        if !more {
            inner.disk_flushing = false;
        }
        more
    }

    /// Go `scheduleCheckpointFlushLocked`: persist completion bits shortly
    /// after pieces finish, so a killed process resumes without rehashing.
    fn schedule_checkpoint_flush_locked(&self, inner: &mut StorageInner) {
        if inner.stream
            || inner.closed
            || !inner.attached
            || inner.checkpoint_flushing
            || inner.checkpoint_flush_scheduled
        {
            return;
        }
        if !inner.checkpoint_dirty && inner.pending_sync_fds.is_empty() {
            return;
        }
        let weak = self.weak_self.lock().clone();
        let delay = CHECKPOINT_FLUSH_DELAY + inner.checkpoint_retry;
        inner.checkpoint_flush_scheduled = true;
        let spawned = std::thread::Builder::new()
            .name("wt-checkpoint".into())
            .spawn(move || {
                std::thread::sleep(delay);
                if let Some(storage) = weak.upgrade() {
                    storage.run_scheduled_checkpoint_flush();
                }
            });
        if spawned.is_err() {
            inner.checkpoint_flush_scheduled = false;
        }
    }

    fn run_scheduled_checkpoint_flush(&self) {
        {
            let mut inner = self.inner.lock();
            inner.checkpoint_flush_scheduled = false;
            if inner.closed || !inner.attached || inner.stream || inner.checkpoint_flushing {
                return;
            }
            if !inner.checkpoint_dirty && inner.pending_sync_fds.is_empty() {
                return;
            }
            inner.checkpoint_flushing = true;
        }
        let ok = self.drain_sync_and_checkpoint_with(false);
        let mut inner = self.inner.lock();
        inner.checkpoint_flushing = false;
        self.startup_drained.notify_all();
        if inner.closed {
            return;
        }
        if ok {
            inner.checkpoint_failures = 0;
            inner.checkpoint_retry = Duration::ZERO;
        } else {
            inner.checkpoint_failures += 1;
            let shift = (inner.checkpoint_failures - 1).min(5);
            inner.checkpoint_retry =
                (Duration::from_millis(100) * (1u32 << shift)).min(Duration::from_secs(30));
        }
        self.schedule_checkpoint_flush_locked(&mut inner);
    }

    /// Block until no scheduled checkpoint publish is in flight (Go waits on
    /// `checkpointFlushing` before Close/MarkVerified rewrite the file).
    fn wait_checkpoint_idle_locked(&self, inner: &mut parking_lot::MutexGuard<'_, StorageInner>) {
        while inner.checkpoint_flushing || inner.startup_writing {
            self.startup_drained.wait(inner);
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
        inner.files[index].file = Some(Arc::new(file));
        inner.files_gen += 1;
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
        self.wait_checkpoint_idle_locked(&mut inner);
        inner.needs_verify = false;
        inner.checkpoint_dirty = true;
        inner.checkpoint_seq += 1;
        drop(self.drain_now(inner));
    }

    /// Go Close/MarkVerified: drain immediately with `checkpoint_flushing`
    /// held so no scheduled drain interleaves, but fsync and write unlocked.
    /// Callers have waited for [`Self::wait_checkpoint_idle_locked`].
    fn drain_now<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
    ) -> parking_lot::MutexGuard<'a, StorageInner> {
        inner.checkpoint_flushing = true;
        drop(inner);
        let _ = self.drain_sync_and_checkpoint_with(false);
        let mut inner = self.inner.lock();
        inner.checkpoint_flushing = false;
        self.startup_drained.notify_all();
        inner
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
            if !inner.closed && inner.attached && !inner.stream {
                inner.max_cache_bytes = saved_download_cache_bytes(bytes, inner.chunk_length);
            }
        }
    }

    /// One line for the per-second transfer log.
    pub fn probe_line(&self) -> String {
        let inner = self.inner.lock();
        format!(
            "stream={} ram={}M/{}M slots={} dirty={} partial_slots={} flushing={} evicting={} sync_pending={} write_err={:?}",
            inner.stream,
            inner.cache_bytes >> 20,
            inner.max_cache_bytes >> 20,
            inner.cache.len(),
            inner.cache_dirty.values().filter(|&&d| d).count(),
            inner.partial_slots.len(),
            inner.flushing.len(),
            inner.evicting.len(),
            inner.pending_sync_fds.len(),
            inner.flush_err,
        )
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
        inner.files_gen += 1;
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
            inner.files[i].file = f.map(Arc::new);
            inner.files[i].external = descriptors[i].is_some();
        }
        inner.files_gen += 1;
        inner.stream = false;
        inner.attached = true;
        inner.max_cache_bytes = saved_download_cache_bytes(inner.download_max_bytes, inner.chunk_length);

        // Flush prepared cache data to disk
        let prepared_spans = inner.dirty_spans.clone();
        let cache_items: Vec<(usize, Arc<Vec<u8>>)> =
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
        inner.flush_new.clear();

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
        inner.flush_new.clear();
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
        self.wait_checkpoint_idle_locked(&mut inner);
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

        // Go UpdateSelection. Handles are replaced only after every step
        // succeeds; on error the opened duplicates are dropped (closed).
        let mut opened: Vec<(usize, File)> = Vec::new();
        for (i, descriptor) in descriptors.iter().enumerate() {
            let old = &inner.files[i];
            // A selected boundary file needs an Android destination (download
            // mode). Stream mode keeps engine-owned handles; a selected external
            // file may omit its descriptor to keep the existing handle.
            if !chosen.contains(&i) || (old.external || inner.stream) && descriptor.is_none() {
                continue;
            }
            if inner.stream {
                return Err(StorageError::Other(format!(
                    "stream storage rejects external descriptor for file {}",
                    i
                )));
            }
            let fd = match *descriptor {
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
            // Android may resend descriptors for every selected file. The
            // already owned destination must keep its bytes: truncating it
            // would zero pieces that stay marked complete.
            if same_file(old.file.as_deref(), &file) {
                continue;
            }
            // A new destination is rebuilt from verified pieces below rather
            // than trusting arbitrary stale bytes.
            file.set_len(0)?;
            file.set_len(old.length as u64)?;
            opened.push((i, file));
        }

        // Copy only pieces known complete; partial data is fetched and
        // hash-checked again after reselection.
        for (i, file) in &opened {
            let (offset, length) = (inner.files[*i].offset, inner.files[*i].length);
            for (&p, &is_comp) in &inner.completed {
                if !is_comp {
                    continue;
                }
                let piece_len = inner.info.as_ref().unwrap().piece(p as u32).length;
                let piece_start = (p as i64) * inner.chunk_length;
                let from = piece_start.max(offset);
                let to = (piece_start + piece_len).min(offset + length);
                if from >= to {
                    continue;
                }
                let mut data = vec![0u8; (to - from) as usize];
                Self::read_from_files_locked(&inner.files, from, &mut data)?;
                file.write_all_at(&data, (from - offset) as u64)?;
            }
        }

        let replaced: Vec<usize> = opened.iter().map(|(i, _)| *i).collect();
        for (i, file) in opened {
            inner.files[i].file = Some(Arc::new(file));
            inner.files[i].external = true;
        }
        if !replaced.is_empty() {
            inner.files_gen += 1;
        }
        // Go clearCompletedOverlappingLocked: a stale checkpoint must not let
        // pieces touching a replaced file skip hashing later.
        for i in replaced {
            let (offset, length) = (inner.files[i].offset, inner.files[i].length);
            let chunk = inner.chunk_length;
            let info = inner.info.clone();
            inner.completed.retain(|&p, _| {
                let start = p as i64 * chunk;
                let end = start + info.as_ref().map_or(0, |info| info.piece(p as u32).length);
                end <= offset || start >= offset + length
            });
        }
        self.capture_identities_locked(&mut inner);
        inner.checkpoint_dirty = true;
        inner.checkpoint_seq += 1;
        self.schedule_checkpoint_flush_locked(&mut inner);
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
        let piece = inner.info.as_ref().unwrap().piece(index as u32);
        match self.read_piece_data_unlocked(inner, piece, buf, abs % chunk_len) {
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
        if inner.stream_lost.get(&index).copied().unwrap_or(false) {
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
                Arc::make_mut(cache_slot).copy_from_slice(buf);
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
        if let Some(cb) = callback.filter(|_| !pending_refresh.is_empty()) {
            std::thread::spawn(move || {
                for v in pending_refresh {
                    cb(v);
                }
            });
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
        inner.flush_new.clear();
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
        let inner = self.inner.lock();
        if inner.closed || !inner.attached || inner.chunk_length <= 0 || buf.is_empty() {
            return (0, false);
        }
        let index = (abs / inner.chunk_length) as usize;
        if !inner.completed.get(&index).copied().unwrap_or(false) {
            return (0, false);
        }
        let chunk_len = inner.chunk_length;
        let piece = inner.info.as_ref().unwrap().piece(index as u32);
        match self.read_piece_data_unlocked(inner, piece, buf, abs % chunk_len) {
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
    ) -> StorageResult<CacheSlot> {
        if let Some(existing) = inner.cache.get(&index) {
            if existing.len() as i64 == expected {
                inner.cache_order.touch(index as i32);
                return Ok(CacheSlot::Ready(false));
            }
            self.drop_cache_locked(inner, index);
        }

        while inner.cache_bytes + expected > inner.max_cache_bytes {
            let mut victim = None;
            // Verified pieces first: an unverified one evicted here is read
            // straight back from disk to be hashed or to finish receiving.
            for verified_only in [true, false] {
                if victim.is_some() {
                    break;
                }
                inner.cache_order.each(|p| {
                    let p_idx = p as usize;
                    // A flushing slot holds bytes not yet published to disk.
                    if p_idx == index || inner.flushing.contains(&p_idx) {
                        return true;
                    }
                    if inner.cache_dirty.get(&p_idx).copied().unwrap_or(false) {
                        return true;
                    }
                    if verified_only && !inner.completed.get(&p_idx).copied().unwrap_or(false) {
                        return true;
                    }
                    if !verified_only {
                        wt_types::probe::add(wt_types::probe::EVICT_UNVERIFIED, 1);
                    }
                    victim = Some(p_idx);
                    false
                });
            }

            if let Some(v) = victim {
                self.drop_cache_locked(inner, v);
            } else {
                let mut dirty_victim = None;
                inner.cache_order.each(|p| {
                    let p_idx = p as usize;
                    if p_idx != index && !inner.flushing.contains(&p_idx) {
                        dirty_victim = Some(p_idx);
                        return false;
                    }
                    true
                });
                if let Some(dv) = dirty_victim {
                    // The caller writes it back unlocked, then retries.
                    return Ok(CacheSlot::FlushFirst(dv));
                } else {
                    return Err(StorageError::Other("cache full of unflushable data".into()));
                }
            }
        }

        inner.cache.insert(index, Arc::new(vec![0u8; expected as usize]));
        inner.cache_bytes += expected;
        inner.cache_order.touch(index as i32);
        Ok(CacheSlot::Ready(true))
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

        inner.cache.insert(index, Arc::new(vec![0u8; expected as usize]));
        inner.cache_bytes += expected;
        inner.stream_revision += 1;
        inner.cache_order.touch(index as i32);
        Ok((true, refresh))
    }

    fn drop_cache_locked(&self, inner: &mut StorageInner, index: usize) {
        if let Some(data) = inner.cache.remove(&index) {
            inner.cache_bytes -= data.len() as i64;
            inner.cache_dirty.remove(&index);
            // Spans describe this buffer. Stale ones would make a later slot
            // flush zeros for ranges it never received.
            inner.dirty_spans.remove(&index);
            inner.flush_new.remove(&index);
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
        if !inner.dirty_spans.contains_key(&index) && inner.partial_slots.contains_key(&index) {
            // Go: unwritten bytes of a partial slot are zeros, not the piece.
            inner.cache_dirty.insert(index, false);
            return Ok(());
        }
        let spans = inner.dirty_spans.remove(&index).unwrap_or_else(|| {
            vec![ByteRange {
                start: 0,
                end: data.len() as i64,
            }]
        });

        for (i, span) in spans.iter().enumerate() {
            let start = (index as i64) * inner.chunk_length + span.start;
            let chunk = &data[span.start as usize..span.end as usize];
            if let Err(e) = Self::write_to_files_locked(&mut inner.files, start, chunk) {
                // Keep this and later spans dirty. Earlier spans are on disk;
                // a retry must neither lose these nor rewrite the whole piece.
                inner.dirty_spans.insert(index, spans[i..].to_vec());
                inner.flush_err = Some(e.to_string());
                return Err(e);
            }
        }

        inner.cache_dirty.insert(index, false);
        self.mark_saved_partial_locked(inner, index);
        inner.flush_err = None;
        Ok(())
    }

    fn mark_saved_partial_locked(&self, inner: &mut StorageInner, index: usize) {
        if !inner.completed.get(&index).copied().unwrap_or(false) {
            inner.saved_partial.insert(index, true);
        }
    }

    /// Plan an unlocked write of `index`'s dirty spans, mirroring
    /// [`Self::write_to_files_locked`] bounds checks. `None` when clean.
    fn plan_piece_flush_locked(
        &self,
        inner: &StorageInner,
        index: usize,
    ) -> StorageResult<Option<PieceFlush>> {
        if !inner.cache_dirty.get(&index).copied().unwrap_or(false) {
            return Ok(None);
        }
        let Some(data) = inner.cache.get(&index) else {
            return Ok(None);
        };
        if !inner.dirty_spans.contains_key(&index) && inner.partial_slots.contains_key(&index) {
            // Go: unwritten bytes of a partial slot are zeros, not the piece.
            return Ok(Some(PieceFlush {
                writes: Vec::new(),
                files_gen: inner.files_gen,
            }));
        }
        let whole = [ByteRange {
            start: 0,
            end: data.len() as i64,
        }];
        let spans = inner.dirty_spans.get(&index).map_or(&whole[..], |s| &s[..]);
        let mut writes = Vec::new();
        for span in spans {
            let start = (index as i64) * inner.chunk_length + span.start;
            let n = (span.end - span.start) as usize;
            for (file, at, lo, hi) in plan_file_reads(&inner.files, start, n)? {
                let base = span.start as usize;
                writes.push((file, at, Arc::clone(data), base + lo, base + hi));
            }
        }
        Ok(Some(PieceFlush {
            writes,
            files_gen: inner.files_gen,
        }))
    }

    /// Write `index`'s dirty spans without holding the lock. The slot cannot
    /// be evicted meanwhile. `Ok(true)`: the cache bytes are on disk and the
    /// slot is clean. `Ok(false)`: a write arrived or files were replaced
    /// meanwhile; its spans stay dirty for another flush. Errors keep the
    /// spans dirty.
    fn flush_piece_unlocked<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        index: usize,
    ) -> (
        parking_lot::MutexGuard<'a, StorageInner>,
        StorageResult<bool>,
    ) {
        // One unlocked writer per piece; the other re-plans afterwards.
        while inner.flushing.contains(&index) {
            self.flush_done.wait(&mut inner);
        }
        if inner.closed {
            return (inner, Err(StorageError::Closed));
        }
        let plan = match self.plan_piece_flush_locked(&inner, index) {
            Ok(Some(plan)) => plan,
            Ok(None) => return (inner, Ok(true)),
            Err(e) => {
                inner.flush_err = Some(e.to_string());
                return (inner, Err(e));
            }
        };
        inner.flushing.insert(index);
        inner.flush_raced.remove(&index);
        inner.flush_new.remove(&index);
        drop(inner);
        let written = plan.write();
        let mut inner = self.inner.lock();
        inner.flushing.remove(&index);
        self.flush_done.notify_all();
        let raced = inner.flush_raced.remove(&index);
        let arrived = inner.flush_new.remove(&index);
        if inner.closed {
            return (inner, Err(StorageError::Closed));
        }
        if let Err(e) = written {
            inner.flush_err = Some(e.to_string());
            return (inner, Err(e));
        }
        if inner.files_gen != plan.files_gen {
            // Files were replaced: every span stays dirty for the new files.
            // Escalating to the whole slot would write a partial slot's
            // zeros over data saved before an eviction.
            return (inner, Ok(false));
        }
        if !plan.writes.is_empty() {
            self.mark_saved_partial_locked(&mut inner, index);
        }
        if raced {
            // Blocks are never rewritten (core rejects duplicates) and
            // flushes of one piece are serialized, so the written bytes are
            // current: only blocks that arrived during the write are still
            // unsaved. Keeping every span dirty made each later flush
            // rewrite all the piece had received so far, again and again
            // while it downloaded.
            if let Some(arrived) =
                arrived.filter(|a| !a.is_empty() && inner.cache.contains_key(&index))
            {
                inner.dirty_spans.insert(index, arrived);
                inner.cache_dirty.insert(index, true);
                inner.flush_err = None;
                return (inner, Ok(false));
            }
        }
        inner.cache_dirty.insert(index, false);
        inner.dirty_spans.remove(&index);
        inner.flush_err = None;
        (inner, Ok(true))
    }

    /// Completion must drain racing writes too. Each retry replans under the
    /// mutex and performs the actual I/O after releasing it.
    fn flush_piece_until_clean<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        index: usize,
    ) -> (parking_lot::MutexGuard<'a, StorageInner>, StorageResult<()>) {
        loop {
            let (guard, result) = self.flush_piece_unlocked(inner, index);
            inner = guard;
            match result {
                Ok(true) => return (inner, Ok(())),
                Ok(false) => continue,
                Err(e) => return (inner, Err(e)),
            }
        }
    }

    /// Bounded-cache fallback. Reserve the uncached piece while writing so
    /// another receiver cannot create a stale slot or publish an older write
    /// over this one. Duplicated handles keep the unlocked write valid.
    fn write_through_unlocked<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        index: usize,
        off: i64,
        data: &[u8],
    ) -> (
        parking_lot::MutexGuard<'a, StorageInner>,
        StorageResult<usize>,
    ) {
        wt_types::probe::add(wt_types::probe::WRITE_THROUGH, 1);
        inner.flushing.insert(index);
        let result = loop {
            if inner.closed {
                break Err(StorageError::Closed);
            }
            if !inner.attached || inner.stream {
                break Err(StorageError::Unavailable);
            }
            let start = index as i64 * inner.chunk_length + off;
            let writes = match plan_file_reads(&inner.files, start, data.len()) {
                Ok(writes) => writes
                    .into_iter()
                    .map(|(file, at, lo, hi)| (file, at, Arc::new(data[lo..hi].to_vec()), 0, hi - lo))
                    .collect(),
                Err(e) => break Err(e),
            };
            let plan = PieceFlush {
                writes,
                files_gen: inner.files_gen,
            };
            drop(inner);
            let written = plan.write();
            inner = self.inner.lock();
            if inner.closed {
                break Err(StorageError::Closed);
            }
            if inner.files_gen != plan.files_gen {
                continue;
            }
            match written {
                Ok(()) => {
                    self.mark_saved_partial_locked(&mut inner, index);
                    break Ok(data.len());
                }
                Err(e) => break Err(e),
            }
        };
        inner.flushing.remove(&index);
        self.flush_done.notify_all();
        (inner, result)
    }

    /// Fill a partial cache slot with the flush and disk read done unlocked.
    /// Bytes written meanwhile (tracked in `partial_slots`) win over
    /// the disk copy.
    fn fill_partial_slot_unlocked<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        index: usize,
        off: i64,
        n: i64,
    ) -> (parking_lot::MutexGuard<'a, StorageInner>, StorageResult<()>) {
        match inner.partial_slots.get(&index) {
            Some(spans) if !ranges_cover(spans, off, off + n) => {}
            _ => return (inner, Ok(())),
        }
        let (guard, flushed) = self.flush_piece_unlocked(inner, index);
        inner = guard;
        // Racing received bytes are overlaid below; they need not be forced
        // to disk just to fill the slot's missing ranges.
        let flushed = flushed.map(|_| ());
        if let Err(e) = flushed {
            return (inner, Err(e));
        }
        let Some(len) = inner.cache.get(&index).map(|slot| slot.len()) else {
            return (inner, Ok(()));
        };
        if !inner.partial_slots.contains_key(&index) {
            return (inner, Ok(()));
        }
        let start = (index as i64) * inner.chunk_length;
        let reads = match plan_file_reads(&inner.files, start, len) {
            Ok(r) => r,
            Err(e) => return (inner, Err(e)),
        };
        let files_gen = inner.files_gen;
        drop(inner);
        let mut disk = vec![0u8; len];
        let mut read: StorageResult<()> = Ok(());
        for (file, at, lo, hi) in reads {
            #[cfg(test)]
            writeback_tests::before_io(false);
            if let Err(e) = file.read_exact_at(&mut disk[lo..hi], at) {
                read = Err(e.into());
                break;
            }
        }
        let mut inner = self.inner.lock();
        if inner.closed {
            return (inner, Err(StorageError::Closed));
        }
        if inner.files_gen != files_gen {
            // Keep the partial slot so a retry fills from the new files.
            return (inner, Err(StorageError::Unavailable));
        }
        if let Err(e) = read {
            return (inner, Err(e));
        }
        // The slot may have been dropped or replaced while unlocked.
        let Some(kept) = inner.partial_slots.get(&index).cloned() else {
            return (inner, Ok(()));
        };
        if let Some(buf) = inner.cache.get_mut(&index).filter(|b| b.len() == len) {
            let buf = Arc::make_mut(buf);
            let mut at = 0usize;
            for span in kept
                .iter()
                .map(|r| (r.start as usize, r.end as usize))
                .chain(std::iter::once((len, len)))
            {
                if span.0 > at {
                    buf[at..span.0].copy_from_slice(&disk[at..span.0]);
                }
                at = at.max(span.1);
            }
            inner.partial_slots.remove(&index);
        }
        (inner, Ok(()))
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
        for f in files_overlapping(&inner.files, start, end) {
            if let Some(file) = &f.file {
                inner.pending_sync_fds.insert(file.as_raw_fd());
            }
        }
    }

    fn capture_identities_locked(&self, inner: &mut StorageInner) {
        let mut ids = Vec::with_capacity(inner.files.len());
        for f in &inner.files {
            if let Some(file) = &f.file {
                #[cfg(test)]
                writeback_tests::before_io(false);
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
        for i in overlapping_file_range(&inner.files, start, end) {
            if !file_stable(&saved[i], &current[i]) {
                return false;
            }
        }
        true
    }

    /// Cache, preview and playback reads share the same unlocked I/O path.
    /// Descriptor snapshots keep positional reads valid through close/replacement.
    fn read_piece_data_unlocked<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        piece: Piece,
        buf: &mut [u8],
        off: i64,
    ) -> StorageResult<usize> {
        if inner.closed {
            return Err(StorageError::Closed);
        }
        if off < 0 || off >= piece.length {
            return Ok(0);
        }
        let index = piece.index as usize;
        if inner.stream_lost.get(&index).copied().unwrap_or(false) {
            return Err(StorageError::Unavailable);
        }

        if inner.cache.contains_key(&index) {
            let (guard, filled) =
                self.fill_partial_slot_unlocked(inner, index, off, buf.len() as i64);
            inner = guard;
            filled?;
            inner.cache_order.touch(index as i32);
            if let Some(data) = inner.cache.get(&index) {
                if off >= data.len() as i64 {
                    return Ok(0);
                }
                let n = ((data.len() as i64 - off) as usize).min(buf.len());
                // A whole piece for the hash is copied unlocked, so peers keep
                // landing blocks meanwhile. Stream slots are never shared.
                if !inner.stream && n >= LARGE_READ_BYTES {
                    let data = Arc::clone(data);
                    drop(inner);
                    buf[..n].copy_from_slice(&data[off as usize..off as usize + n]);
                    return Ok(n);
                }
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
            let path = self.startup_piece_path(index);
            let n = ((piece.length - off) as usize).min(buf.len());
            drop(inner);
            let file = File::open(path)?;
            #[cfg(test)]
            writeback_tests::before_io(false);
            file.read_exact_at(&mut buf[..n], (32 + off) as u64)?;
            return Ok(n);
        }

        let abs = (index as i64) * inner.chunk_length + off;
        let length = piece.length;
        if off < 0 || off >= length {
            return Ok(0);
        }
        let n = ((length - off) as usize).min(buf.len());
        // Bytes outside the cache are on disk and no longer change; read them
        // unlocked so playback and hashing do not stall peers' cache writes.
        let reads = plan_file_reads(&inner.files, abs, n)?;
        drop(inner);
        wt_types::probe::add(wt_types::probe::DISK_READ_KIB, n as u64 / 1024);
        for (file, at, lo, hi) in reads {
            #[cfg(test)]
            writeback_tests::before_io(false);
            file.read_exact_at(&mut buf[lo..hi], at)?;
        }
        Ok(n)
    }

    pub fn write_to_files_locked(
        files: &mut [FileDescriptor],
        start: i64,
        data: &[u8],
    ) -> StorageResult<usize> {
        let end = start + data.len() as i64;
        let mut done = 0usize;
        let range = overlapping_file_range(files, start, end);
        for f in files[range].iter_mut() {
            let from = start.max(f.offset);
            let to = end.min(f.offset + f.length);
            if from >= to {
                continue;
            }
            let file = f.file.as_mut().ok_or_else(|| {
                StorageError::Other(format!("storage for {} is not configured", f.path))
            })?;
            let chunk = &data[(from - start) as usize..(to - start) as usize];
            #[cfg(test)]
            writeback_tests::before_io(true);
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
        for f in files_overlapping(files, start, end) {
            let from = start.max(f.offset);
            let to = end.min(f.offset + f.length);
            if from >= to {
                continue;
            }
            let file = f.file.as_ref().ok_or_else(|| {
                StorageError::Other(format!("storage for {} is not configured", f.path))
            })?;
            let chunk = &mut buf[(from - start) as usize..(to - start) as usize];
            #[cfg(test)]
            writeback_tests::before_io(false);
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
        inner.startup_pending.insert(index, data.to_vec());
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
                if inner.cache.get(&index).map(|slot| slot.as_ref()) != Some(&data) {
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
        // Also drain a scheduled checkpoint publish: it must not finish
        // after Close and delete or overwrite the final checkpoint.
        self.wait_checkpoint_idle_locked(&mut inner);
        if inner.closed {
            return Ok(());
        }
        let mut flush_res = Ok(());
        if inner.attached && !inner.stream {
            flush_res = self.flush_all_dirty_locked(&mut inner);
            inner.checkpoint_dirty = true;
            inner.checkpoint_seq += 1;
            inner = self.drain_now(inner);
            // Another close may have run while the lock was released.
            self.wait_checkpoint_idle_locked(&mut inner);
            if inner.closed {
                return flush_res;
            }
        }
        inner.closed = true;
        inner.evicting.clear();
        inner.checkpoint_gen = inner.checkpoint_gen.wrapping_add(1);
        inner.stream_ready.notify();
        self.flush_done.notify_all();
        if inner.stream {
            crate::direct_storage::unregister_startup_cache(
                &self.base_dir,
                &self.info_hash.to_hex(),
            );
        }
        for f in &mut inner.files {
            let _ = f.file.take();
        }
        inner.files_gen += 1;
        inner.attached = false;
        inner.cache.clear();
        inner.cache_dirty.clear();
        inner.completed.clear();
        inner.cache_bytes = 0;
        inner.dirty_spans.clear();
        inner.flush_new.clear();
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

impl DocumentPiece {
    /// A write into an existing slot is a copy. Creating a slot may first
    /// write a victim back to disk, and a slot being evicted waits for it.
    fn write_stays_in_memory_locked(&self, inner: &StorageInner) -> bool {
        if inner.closed || !inner.attached {
            // Closed fails at once; prepared data lives in RAM only.
            return true;
        }
        let index = self.piece.index as usize;
        inner
            .cache
            .get(&index)
            .is_some_and(|slot| slot.len() as i64 == self.piece.length)
            && !inner.evicting.contains(&index)
    }

    fn write_locked<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        buf: &[u8],
        off: i64,
    ) -> StorageResult<usize> {
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
                inner.cache.insert(index, Arc::new(vec![0u8; expected as usize]));
                inner.cache_bytes += expected;
                inner.cache_order.touch(index as i32);
            }
            let data = Arc::make_mut(inner.cache.get_mut(&index).unwrap());
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
            let mut attempts = 0;
            loop {
                attempts += 1;
                // An eviction flush must finish clean. New bytes wait outside
                // the slot; background flushes of other pieces do not.
                while inner.evicting.contains(&index) {
                    self.storage.flush_done.wait(&mut inner);
                }
                // An uncached write-through owns this piece until its I/O
                // finishes. Waiting releases the mutex for other peers.
                while !inner.cache.contains_key(&index) && inner.flushing.contains(&index) {
                    self.storage.flush_done.wait(&mut inner);
                }
                if inner.closed {
                    return Err(StorageError::Closed);
                }
                let victim = match self
                    .storage
                    .ensure_cache_slot_locked(&mut inner, index, expected)
                {
                    Ok(CacheSlot::Ready(c)) => break c,
                    Ok(CacheSlot::FlushFirst(victim)) if attempts <= 8 => Some(victim),
                    _ => None,
                };
                // Write the dirty victim back without the lock, so other
                // peers' cache writes and reads continue meanwhile.
                // Freeze the victim first: a receive into it during the
                // unlocked write used to keep it dirty forever, the cache
                // never freed, and this block fell through as a 16 KiB
                // write. Go flushes under st.mu so the slot can always be
                // dropped before the next receive.
                let evicted = match victim {
                    Some(victim) => {
                        // RAM is all unfinished pieces: a peer now waits on disk.
                        wt_types::probe::add(wt_types::probe::CACHE_FULL, 1);
                        wt_types::events::record("cache_full_wait", format!("piece={victim}"));
                        inner.evicting.insert(victim);
                        let (guard, flushed) = self.storage.flush_piece_until_clean(inner, victim);
                        inner = guard;
                        if inner.closed {
                            inner.evicting.remove(&victim);
                            self.storage.flush_done.notify_all();
                            return Err(StorageError::Closed);
                        }
                        let clean = flushed.is_ok()
                            && !inner.cache_dirty.get(&victim).copied().unwrap_or(false)
                            && !inner.flushing.contains(&victim);
                        if clean {
                            self.storage.drop_cache_locked(&mut inner, victim);
                        }
                        inner.evicting.remove(&victim);
                        self.storage.flush_done.notify_all();
                        clean
                    }
                    None => false,
                };
                if !evicted {
                    // The victim flush released the mutex. Another writer may
                    // now own this piece or have created its cache slot.
                    if inner.cache.contains_key(&index) || inner.flushing.contains(&index) {
                        continue;
                    }
                    // Cache full of unflushable data: write through to disk.
                    let (_guard, result) =
                        self.storage.write_through_unlocked(inner, index, off, buf);
                    return result;
                }
            }
        };

        if created && !inner.stream {
            // A piece partly saved before its slot was evicted gets a partial
            // slot like a new one: flushes write only received ranges, so the
            // zeros never reach the disk, and a read that needs the saved
            // bytes loads them unlocked (fill_partial_slot_unlocked). Reading
            // the whole piece here, under the lock every peer's write takes,
            // ran once per evicted piece per block on Android's FUSE storage.
            inner.partial_slots.insert(index, Vec::new());
        }

        // A slot being written back unlocked now holds newer bytes than its
        // snapshot (the lock may have been released above).
        if inner.flushing.contains(&index) {
            inner.flush_raced.insert(index);
            merge_received_range(
                inner.flush_new.entry(index).or_default(),
                ByteRange {
                    start: off,
                    end: off + buf.len() as i64,
                },
            );
        }
        if let Some(data) = inner.cache.get_mut(&index) {
            Arc::make_mut(data)[off as usize..off as usize + buf.len()].copy_from_slice(buf);
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
        if let Some(cb) = callback.filter(|_| !pending_refresh.is_empty()) {
            std::thread::spawn(move || {
                for v in pending_refresh {
                    cb(v);
                }
            });
        }

        Ok(buf.len())
    }
}

impl PieceStorage for DocumentPiece {
    fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize> {
        self.storage
            .read_piece_data_unlocked(self.storage.inner.lock(), self.piece, buf, off)
    }

    fn read_at_in_memory(&self, buf: &mut [u8], off: i64) -> Option<StorageResult<usize>> {
        let inner = self.storage.inner.lock();
        let index = self.piece.index as usize;
        if inner.closed || off < 0 {
            return None;
        }
        let data = inner.cache.get(&index)?;
        let end = off + buf.len() as i64;
        if end > data.len() as i64
            || inner.stream_lost.get(&index).copied().unwrap_or(false)
            || inner.partial_slots.get(&index).is_some_and(|spans| !ranges_cover(spans, off, end))
        {
            return None;
        }
        buf.copy_from_slice(&data[off as usize..end as usize]);
        Some(Ok(buf.len()))
    }

    fn write_at_in_memory(&self, buf: &[u8], off: i64) -> Option<StorageResult<usize>> {
        let waiting = std::time::Instant::now();
        let inner = self.storage.inner.lock();
        wt_types::probe::max(wt_types::probe::LOCK_WAIT_MAX_US, waiting.elapsed().as_micros() as u64);
        if !self.write_stays_in_memory_locked(&inner) {
            return None;
        }
        Some(self.write_locked(inner, buf, off))
    }

    fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize> {
        self.write_locked(self.storage.inner.lock(), buf, off)
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

        // Download mode: the piece is whole and verified in RAM, so it goes to
        // disk in one write, without the lock, so peers keep filling RAM. A
        // piece that fails its hash never gets here and never reaches the disk.
        let write_started = std::time::Instant::now();
        let (guard, flushed) = self.storage.flush_piece_until_clean(inner, index);
        inner = guard;
        flushed?;
        let write_ms = write_started.elapsed().as_millis() as u64;
        wt_types::probe::add(wt_types::probe::DISK_WRITES, 1);
        wt_types::probe::add(wt_types::probe::DISK_WRITE_KIB, self.piece.length as u64 / 1024);
        wt_types::probe::add(wt_types::probe::DISK_WRITE_MS, write_ms);
        wt_types::probe::max(wt_types::probe::DISK_WRITE_MAX_MS, write_ms);
        if write_started.elapsed() >= Duration::from_millis(100) {
            wt_types::events::record("disk_write", format!(
                "piece={index} {}KiB {}ms", self.piece.length / 1024, write_started.elapsed().as_millis()));
        }
        inner.completed.insert(index, true);
        inner.saved_partial.remove(&index);
        self.storage
            .note_piece_files_need_sync_locked(&mut inner, index);
        inner.checkpoint_dirty = true;
        inner.checkpoint_seq += 1;
        self.storage.schedule_checkpoint_flush_locked(&mut inner);
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
        inner.checkpoint_seq += 1;
        self.storage.schedule_checkpoint_flush_locked(&mut inner);
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

/// Go `sameFile`: both handles name the same device/inode.
fn same_file(a: Option<&File>, b: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(a) = a else {
        return false;
    };
    match (a.metadata(), b.metadata()) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// Duplicated descriptors and positions for reading `[start, start + n)`
/// unlocked: `(file, file_offset, buf_start, buf_end)`.
fn plan_file_reads(
    files: &[FileDescriptor],
    start: i64,
    n: usize,
) -> StorageResult<Vec<(Arc<File>, u64, usize, usize)>> {
    let end = start + n as i64;
    let mut reads = Vec::new();
    let mut done = 0usize;
    for f in files_overlapping(files, start, end) {
        let from = start.max(f.offset);
        let to = end.min(f.offset + f.length);
        if from >= to {
            continue;
        }
        let file = f.file.as_ref().ok_or_else(|| {
            StorageError::Other(format!("storage for {} is not configured", f.path))
        })?;
        let lo = (from - start) as usize;
        let hi = (to - start) as usize;
        reads.push((Arc::clone(file), (from - f.offset) as u64, lo, hi));
        done += hi - lo;
    }
    if done != n {
        return Err(StorageError::Other("short read from files".into()));
    }
    Ok(reads)
}

/// [`DocumentTorrentStorage::ensure_cache_slot_locked`] outcome.
enum CacheSlot {
    /// Slot present; `true` when newly created.
    Ready(bool),
    /// Only dirty pieces can be evicted: write this one back first.
    FlushFirst(usize),
}

/// A descriptor and its layout captured before an unlocked metadata query.
struct FileIdentitySnapshot {
    file: Option<File>,
    identity: FileIdentity,
}

impl FileIdentitySnapshot {
    fn new(file: &FileDescriptor) -> std::io::Result<Self> {
        Ok(Self {
            file: file.file.as_ref().map(|f| f.try_clone()).transpose()?,
            identity: FileIdentity {
                external: file.external,
                length: file.length,
                path: file.path.clone(),
                ..Default::default()
            },
        })
    }

    fn capture(self) -> FileIdentity {
        match self.file {
            Some(file) => {
                #[cfg(test)]
                writeback_tests::before_io(false);
                identity_from_file(
                    &file,
                    self.identity.external,
                    self.identity.length,
                    self.identity.path,
                )
                .0
            }
            None => self.identity,
        }
    }
}

/// A piece's dirty bytes, copied with duplicated descriptors so the write can
/// run without the storage lock (and survive file replacement or close).
pub struct PieceFlush {
    writes: Vec<(Arc<File>, u64, Arc<Vec<u8>>, usize, usize)>,
    files_gen: u64,
}

impl PieceFlush {
    fn write(&self) -> StorageResult<()> {
        for (file, at, data, lo, hi) in &self.writes {
            #[cfg(test)]
            writeback_tests::before_io(true);
            file.write_all_at(&data[*lo..*hi], *at)?;
        }
        Ok(())
    }
}

/// Files holding bytes of `[start, end)` (Go `filesOverlapping`).
/// Files are in torrent order; binary search finds the overlapping slice.
pub fn files_overlapping(files: &[FileDescriptor], start: i64, end: i64) -> &[FileDescriptor] {
    &files[overlapping_file_range(files, start, end)]
}

/// Index range of [`files_overlapping`]. File ends never decrease in torrent
/// order, so both searches are monotonic. Zero-length files inside the range
/// are included and skipped by callers' empty-span checks.
pub fn overlapping_file_range(
    files: &[FileDescriptor],
    start: i64,
    end: i64,
) -> std::ops::Range<usize> {
    let first = files.partition_point(|f| f.offset + f.length <= start);
    let last = first + files[first..].partition_point(|f| f.offset < end);
    first..last
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
#[path = "storage_writeback_tests.rs"]
mod writeback_tests;

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
                inner.cache.insert(index, Arc::new(vec![1]));
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

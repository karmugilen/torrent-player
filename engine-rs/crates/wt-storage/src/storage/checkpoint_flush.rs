//! Syncing files and publishing checkpoints.

use super::*;

impl DocumentTorrentStorage {
    /// Public drain entry (Go `drainSyncAndCheckpoint`).
    /// Releases the storage lock around sync/write hooks so test seams can re-enter.
    pub fn drain_sync_and_checkpoint(&self) {
        let _ = self.drain_sync_and_checkpoint_with(true);
    }

    /// `flush_dirty` first writes every cached dirty piece (explicit drains).
    /// Scheduled drains, like Go's, only make completed pieces durable: the
    /// write-back cache keeps partial pieces. Returns false when a sync or
    /// checkpoint write failed and must be retried.
    pub(super) fn drain_sync_and_checkpoint_with(&self, flush_dirty: bool) -> bool {
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
    pub(super) fn run_disk_flush(&self) -> bool {
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
    pub(super) fn schedule_checkpoint_flush_locked(&self, inner: &mut StorageInner) {
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

    pub(super) fn run_scheduled_checkpoint_flush(&self) {
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
    pub(super) fn wait_checkpoint_idle_locked(&self, inner: &mut parking_lot::MutexGuard<'_, StorageInner>) {
        while inner.checkpoint_flushing || inner.startup_writing {
            self.startup_drained.wait(inner);
        }
    }
}

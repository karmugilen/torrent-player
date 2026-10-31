//! One piece's reads, writes and completion.

use super::*;

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

    pub(super) fn hash_contents(&self) -> StorageResult<[u8; 20]> {
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
    pub(super) fn write_stays_in_memory_locked(&self, inner: &StorageInner) -> bool {
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

    pub(super) fn write_locked<'a>(
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
        let inner = self.storage.inner.lock();
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
        let (guard, flushed) = self.storage.flush_piece_until_clean(inner, index);
        inner = guard;
        flushed?;
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

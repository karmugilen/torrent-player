//! RAM slots, and writing them to and reading them from files.

use super::*;

impl DocumentTorrentStorage {
    pub(super) fn ensure_cache_slot_locked(
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
    pub(super) fn ensure_stream_slot_locked(
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

    pub(super) fn drop_cache_locked(&self, inner: &mut StorageInner, index: usize) {
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

    pub(super) fn mark_saved_partial_locked(&self, inner: &mut StorageInner, index: usize) {
        if !inner.completed.get(&index).copied().unwrap_or(false) {
            inner.saved_partial.insert(index, true);
        }
    }

    /// Plan an unlocked write of `index`'s dirty spans, mirroring
    /// [`Self::write_to_files_locked`] bounds checks. `None` when clean.
    pub(super) fn plan_piece_flush_locked(
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
    pub(super) fn flush_piece_unlocked<'a>(
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
    pub(super) fn flush_piece_until_clean<'a>(
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
    pub(super) fn write_through_unlocked<'a>(
        &'a self,
        mut inner: parking_lot::MutexGuard<'a, StorageInner>,
        index: usize,
        off: i64,
        data: &[u8],
    ) -> (
        parking_lot::MutexGuard<'a, StorageInner>,
        StorageResult<usize>,
    ) {
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
    pub(super) fn fill_partial_slot_unlocked<'a>(
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
            super::writeback_tests::before_io(false);
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

    pub(super) fn note_piece_files_need_sync_locked(&self, inner: &mut StorageInner, index: usize) {
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

    pub(super) fn capture_identities_locked(&self, inner: &mut StorageInner) {
        let mut ids = Vec::with_capacity(inner.files.len());
        for f in &inner.files {
            if let Some(file) = &f.file {
                #[cfg(test)]
                super::writeback_tests::before_io(false);
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

    pub(super) fn load_checkpoint_locked(
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

    pub(super) fn piece_files_stable_locked(
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
    pub(super) fn read_piece_data_unlocked<'a>(
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
            super::writeback_tests::before_io(false);
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
        for (file, at, lo, hi) in reads {
            #[cfg(test)]
            super::writeback_tests::before_io(false);
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
            super::writeback_tests::before_io(true);
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
            super::writeback_tests::before_io(false);
            file.read_exact_at(chunk, (from - f.offset) as u64)?;
            done += chunk.len();
        }
        if done != buf.len() {
            return Err(StorageError::Other("short read from files".into()));
        }
        Ok(done)
    }
}

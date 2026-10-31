//! Attaching files and mapping pieces onto them.

use super::*;

impl DocumentTorrentStorage {
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

    pub(super) fn byte_spans_locked(
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
}

//! Direct Play: startup pieces, verified stream reads and preview files.

use super::*;

impl DocumentTorrentStorage {
    pub(super) fn startup_plan_locked(&self, inner: &StorageInner, first_file: Option<usize>) -> Vec<usize> {
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

    pub(super) fn startup_file_on_disk_locked(&self, inner: &StorageInner, index: usize) -> bool {
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

    pub(super) fn validate_startup_pieces_locked(&self, inner: &mut StorageInner, planned: &HashSet<usize>) {
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

    pub(super) fn read_startup_piece_locked(
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

    pub(super) fn valid_startup_piece_locked(&self, inner: &StorageInner, index: usize, data: &[u8]) -> bool {
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

    pub(super) fn read_received_locked(
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

    pub(super) fn release_preview_temp_locked(&self, inner: &mut StorageInner) {
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

    pub(super) fn save_startup_piece_locked(&self, inner: &mut StorageInner, index: usize) {
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

    pub(super) fn drain_startup_pieces(&self) {
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

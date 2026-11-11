//! Verified resume snapshots and debounced publication, ported from playback_resume.go.

use super::{EngineServer, TorrentRecord};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::{atomic::Ordering, Arc};
use std::time::{Duration, SystemTime};
use wt_storage::{ResumeWindowMeta, RESUME_WINDOW_MAX_BYTES, RESUME_WINDOW_TOTAL_MAX};

#[derive(Default)]
pub(super) struct ResumeState {
    pending: HashMap<String, PendingSave>,
    known: HashMap<String, ResumeWindowMeta>,
    generation: u64,
    clear_generation: u64,
}

struct PendingSave {
    record: Arc<RwLock<TorrentRecord>>,
    file_index: usize,
    offset: i64,
    generation: u64,
    clear_generation: u64,
}

impl EngineServer {
    /// Closing a seek schedules one write after two quiet seconds, never inline I/O.
    pub fn schedule_resume_persist(&self, id: &str, file_index: usize, offset: i64) {
        let Some(record) = self.record_by_id(id) else {
            return;
        };
        let hash = record.read().info_hash.clone();
        let generation = {
            let mut state = self.resume_state.lock();
            state.generation += 1;
            let generation = state.generation;
            let clear_generation = state.clear_generation;
            state.pending.insert(
                hash.clone(),
                PendingSave {
                    record,
                    file_index,
                    offset,
                    generation,
                    clear_generation,
                },
            );
            generation
        };
        let weak = self.self_ref.get().cloned().unwrap_or_default();
        self.client.inner.tokio_handle.spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let Some(server) = weak.upgrade() else { return };
            let pending = {
                let mut state = server.resume_state.lock();
                if state
                    .pending
                    .get(&hash)
                    .is_some_and(|p| p.generation == generation)
                    && !server.shutting_down.load(Ordering::SeqCst)
                {
                    state.pending.remove(&hash)
                } else {
                    None
                }
            };
            if let Some(save) = pending {
                // Hashing and fsync must not occupy a reactor thread.
                let _ = tokio::task::spawn_blocking(move || server.persist_resume(save)).await;
            }
        });
    }

    pub fn flush_resume_saves(&self) {
        let pending = std::mem::take(&mut self.resume_state.lock().pending);
        for (_, save) in pending {
            self.persist_resume(save);
        }
    }

    fn persist_resume(&self, save: PendingSave) {
        let clear_generation = save.clear_generation;
        let (torrent, hash) = {
            let record = save.record.read();
            (record.torrent.clone(), record.info_hash.clone())
        };
        if torrent.closed() {
            return;
        }
        let Some(info) = torrent.info() else { return };
        let files = torrent.files();
        let Some(file) = files.get(save.file_index) else {
            return;
        };
        let Some(storage) = self.storage_for_torrent(&torrent) else {
            return;
        };
        if info.piece_length <= 0 {
            return;
        }
        let Some(position) = file.offset().checked_add(save.offset.max(0)) else {
            return;
        };
        let mut abs = position / info.piece_length * info.piece_length;
        if abs < file.offset() {
            abs += info.piece_length;
        }
        let end = (file.offset() + file.length()).min(abs.saturating_add(RESUME_WINDOW_MAX_BYTES));
        let mut length = 0;
        while abs + length < end {
            let index = ((abs + length) / info.piece_length) as u32;
            let piece = info.piece(index);
            if abs + length + piece.length > end || !storage.piece_complete(index as usize) {
                break;
            }
            length += piece.length;
        }
        if length <= 0 {
            return;
        }
        let Some(bytes) = storage.copy_verified_range(abs, length) else {
            return;
        };
        let now = SystemTime::now();
        let meta = ResumeWindowMeta {
            version: wt_storage::RESUME_WINDOW_VERSION,
            info_hash: hash.clone(),
            file_index: save.file_index,
            file_name: file.path(),
            file_length: file.length(),
            piece_length: info.piece_length,
            offset: abs - file.offset(),
            length,
            checksum: wt_storage::resume_checksum(&bytes),
            generation: save.generation,
            saved_at: wt_storage::format_time(now),
            expires: wt_storage::format_time(now + wt_storage::RESUME_WINDOW_TTL),
        };
        // Publication and cache clearing share one lock, so an old snapshot cannot
        // resurrect a cleared window. Keep the same bounded disk budget as Go.
        let protected = self.protected_resume_hashes();
        let mut state = self.resume_state.lock();
        if state.clear_generation != clear_generation {
            return;
        }
        let dir = wt_storage::resume_window_dir(&self.download_dir, &hash);
        let root = self.download_dir.join(".cache/resume");
        let needed =
            bytes.len() as i64 + serde_json::to_vec(&meta).map_or(0, |v| v.len()) as i64 + 4;
        if wt_storage::dir_bytes(&root) - wt_storage::file_size(&dir.join("window.bin")) + needed
            > RESUME_WINDOW_TOTAL_MAX
        {
            self.remove_inactive_resume_windows(&protected);
            state.known.retain(|hash, _| protected.contains(hash));
            if wt_storage::dir_bytes(&root) - wt_storage::file_size(&dir.join("window.bin"))
                + needed
                > RESUME_WINDOW_TOTAL_MAX
            {
                return;
            }
        }
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        if wt_storage::save_resume_window(&self.download_dir, &meta, &bytes).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    dir.join("window.bin"),
                    std::fs::Permissions::from_mode(0o600),
                );
            }
            state.known.insert(hash, meta);
        }
    }

    /// Reuse an already verified RAM range before reading or hashing its disk copy.
    pub fn apply_resume_window_to_storage(&self, id: &str, file_index: usize) -> Option<i64> {
        let record = self.record_by_id(id)?;
        let (torrent, hash) = {
            let record = record.read();
            (record.torrent.clone(), record.info_hash.clone())
        };
        let info = torrent.info()?;
        let files = torrent.files();
        let file = files.get(file_index)?;
        let storage = self.storage_for_torrent(&torrent)?;
        let mut state = self.resume_state.lock();
        let valid = |meta: &ResumeWindowMeta| {
            meta.file_index == file_index
                && meta.offset >= 0
                && meta.length > 0
                && meta
                    .offset
                    .checked_add(meta.length)
                    .is_some_and(|end| end <= file.length())
                && (meta.file_name.is_empty() || meta.file_name == file.path())
                && (meta.file_length == 0 || meta.file_length == file.length())
                && (meta.piece_length == 0 || meta.piece_length == info.piece_length)
                && wt_storage::parse_time(&meta.expires).is_some_and(|end| end > SystemTime::now())
        };
        if let Some(meta) = state.known.get(&hash).filter(|meta| valid(meta)) {
            if storage.range_complete(file.offset() + meta.offset, meta.length) {
                return Some(meta.offset);
            }
        }
        let path = wt_storage::resume_window_dir(&self.download_dir, &hash).join("window.bin");
        if wt_storage::file_size(&path) > RESUME_WINDOW_MAX_BYTES + (64 << 10) + 4 {
            return None;
        }
        let (meta, bytes) = wt_storage::load_resume_window(&self.download_dir, &hash).ok()?;
        if !valid(&meta) {
            return None;
        }
        let abs = file.offset() + meta.offset;
        if !storage.inject_verified_bytes(abs, &bytes) && !storage.range_complete(abs, meta.length)
        {
            return None;
        }
        let offset = meta.offset;
        let begin = abs / info.piece_length;
        let end = (abs + meta.length - 1) / info.piece_length;
        for piece in begin..=end {
            torrent.update_piece_completion(piece as u32);
        }
        state.known.insert(hash, meta);
        Some(offset)
    }

    pub fn delete_resume_window(&self, info_hash: &str) {
        let mut state = self.resume_state.lock();
        state.clear_generation += 1;
        state.pending.remove(info_hash);
        state.known.remove(info_hash);
        let _ = wt_storage::delete_resume_window(&self.download_dir, info_hash);
    }

    pub(super) fn protected_resume_hashes(&self) -> HashSet<String> {
        let reader_ids: HashSet<_> = self
            .playback_readers
            .lock()
            .values()
            .filter(|reader| !reader.is_closed())
            .map(|reader| reader.id.clone())
            .collect();
        let records: Vec<_> = self.records.read().values().cloned().collect();
        records
            .iter()
            .filter_map(|record| {
                let record = record.read();
                (reader_ids.contains(&record.id) || record.playback_coordinator.has_resume_demand())
                    .then(|| record.info_hash.clone())
            })
            .collect()
    }

    fn remove_inactive_resume_windows(&self, protected: &HashSet<String>) {
        let root = self.download_dir.join(".cache/resume");
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && !protected.contains(&entry.file_name().to_string_lossy().into_owned())
                {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }

    pub(super) fn clear_engine_cache(&self, categories: &[String]) -> wt_types::StorageResult<()> {
        if categories.is_empty() {
            return Err(wt_types::StorageError::Other("no categories".into()));
        }
        for category in categories {
            if !["startup", "resume", "peers", "dht"].contains(&category.as_str()) {
                return Err(wt_types::StorageError::Other(format!(
                    "invalid cache category \"{category}\""
                )));
            }
        }
        let protected = self.protected_resume_hashes();
        let mut state = self.resume_state.lock();
        state.clear_generation += 1;
        if categories.iter().any(|category| category == "resume") {
            state.pending.clear();
            state.known.clear();
            self.remove_inactive_resume_windows(&protected);
        }
        drop(state);
        if categories.iter().any(|category| category == "peers") {
            // Clear the live store too, or the next handshake re-persists it.
            let mut hints = self.peer_hints.lock();
            let hashes: Vec<String> = hints.hints_mut().keys().cloned().collect();
            for hash in hashes {
                hints.forget_peer_hints(&self.download_dir, &hash);
            }
        }
        if categories.iter().any(|category| category == "startup") {
            self.clear_inactive_startup_caches();
        }
        let other: Vec<_> = categories
            .iter()
            .filter(|category| !["resume", "startup"].contains(&category.as_str()))
            .cloned()
            .collect();
        if !other.is_empty() {
            wt_storage::clear_cache_categories(&self.download_dir, &other)?;
        }
        Ok(())
    }

    /// Go `clearInactiveStartupCaches`: an attached Direct Play session keeps
    /// its saved startup pieces; storage still serves them.
    fn clear_inactive_startup_caches(&self) {
        let protected: HashSet<String> = self.records.read().values()
            .map(|r| r.read().info_hash.clone())
            .filter(|hash| self.storage.get_storage(hash).is_some_and(|st| st.stream_mode()))
            .collect();
        let root = self.download_dir.join(".startup");
        let Ok(entries) = std::fs::read_dir(&root) else { return; };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() && !protected.contains(&name) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

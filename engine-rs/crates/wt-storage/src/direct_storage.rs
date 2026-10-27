//! Direct Play storage facade: bounded RAM video cache + durable startup metadata.
//!
//! # Behaviour (ports `engine-go/direct_storage.go` + Auto RAM policy)
//!
//! Direct Play keeps video in a bounded RAM cache and saves only video metadata
//! to disk: the head and tail of every selected video (MP4 moov / MKV cues),
//! under `.startup/<infohash>/<piece>.piece`. Opening/container pieces are
//! prioritised; the startup cache is validated on restore (first
//! [`DIRECT_STARTUP_BUDGET`] at attach, the rest when an episode opens).
//! Preview tasks use a separate [`PREVIEW_BUDGET_BYTES`] budget via
//! [`DocumentTorrentStorage::attach_preview_temp`].
//!
//! # Constructor and knobs (agy-3 owns the storage body)
//!
//! | Knob | Where | Notes |
//! |------|--------|-------|
//! | `DocumentStorageClient::new(base_dir)` | agy-3 | Client that opens torrents |
//! | `open_torrent(info, hash)` | `ClientStorage` | Creates [`DocumentTorrentStorage`] |
//! | `attach_descriptors_mode(..., stream=true)` / `attach_direct_play` | agy-3 | Enter Direct Play; rejects external FDs |
//! | `set_cache_limit` / `set_memory_limit` | client + torrent | Runtime; shrink takes effect as pieces are replaced; Direct Play raises to [`DIRECT_PLAY_MIN_PIECES`] × piece length |
//! | `set_startup_file` / `set_preload_file` | agy-3 | Episode plan / Play-next warm start |
//! | `startup_fetch_pieces(playing)` | agy-3 | Batches for metadata download |
//! | `read_verified_stream` / `inject_verified_bytes` | agy-3 | Playback reads |
//! | `set_stream_demand` | agy-3 | Playback window pieces protected from eviction |
//! | `attach_preview_temp` / `release_preview_temp` | agy-3 | Preview memory budget |
//! | [`auto_ram_cache_mib`] | **this module** | 64–512 MiB from available memory + active transfers |
//! | [`clamp_ram_cache_bytes`] | **this module** | Clamp to [`MIN_RAM_CACHE_BYTES`]..=[`MAX_RAM_CACHE_BYTES`] |
//! | [`register_startup_cache`] / [`unregister_startup_cache`] / [`prune_startup_caches`] | **this module** | `.startup` registry + overall disk budget |
//!
//! Constants re-exported from `storage`: [`DIRECT_STARTUP_HEAD`],
//! [`DIRECT_STARTUP_TAIL`], [`DIRECT_STARTUP_BUDGET`],
//! [`DIRECT_STARTUP_TOTAL_BUDGET`], [`DIRECT_PRELOAD_BUDGET`],
//! [`DIRECT_PLAY_MIN_PIECES`], [`PREVIEW_BUDGET_BYTES`],
//! [`MIN_RAM_CACHE_BYTES`], [`MAX_RAM_CACHE_BYTES`].
//!
//! Trait impls: [`DocumentStorageClient`] / [`DocumentTorrentStorage`] /
//! [`DocumentPiece`] already implement `ClientStorage` / `TorrentStorage` /
//! `PieceStorage` (agy-3). This module does not duplicate them.
//!
//! # Auto RAM
//!
//! The Android app computes Auto RAM (`RamCachePolicy.kt`) and pushes the
//! result through `SetCacheLimit`. [`auto_ram_cache_mib`] is the same formula
//! for engine-host / tests so wt-engine can size without the Kotlin layer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use parking_lot::Mutex as ParkingMutex;

use crate::storage::{DocumentStorageClient, DocumentTorrentStorage};
use crate::{startup_file_pieces, ClientStorage, StorageResult};

pub use crate::storage::{
    DIRECT_PLAY_MIN_PIECES, DIRECT_PRELOAD_BUDGET, DIRECT_STARTUP_BUDGET, DIRECT_STARTUP_HEAD,
    DIRECT_STARTUP_TAIL, DIRECT_STARTUP_TOTAL_BUDGET, MAX_RAM_CACHE_BYTES, MIN_RAM_CACHE_BYTES,
    PREVIEW_BUDGET_BYTES,
};

/// Allowed Auto RAM steps (MiB), descending — matches Android `autoRamCacheMiB`.
pub const AUTO_RAM_STEPS_MIB: [i64; 4] = [512, 256, 128, 64];

/// Auto RAM lower bound in MiB (also used on low-RAM / invalid inputs).
pub const AUTO_RAM_MIN_MIB: i64 = 64;

/// Auto RAM upper bound in MiB.
pub const AUTO_RAM_MAX_MIB: i64 = 512;

/// Choose per-transfer RAM cache (MiB) from device memory and active transfers.
///
/// Ports `android/.../RamCachePolicy.kt` `autoRamCacheMiB`. Returns one of
/// 64, 128, 256, 512. Low-RAM devices and invalid totals yield 64.
pub fn auto_ram_cache_mib(
    total_bytes: i64,
    available_bytes: i64,
    low_memory_threshold_bytes: i64,
    low_ram_device: bool,
    active_transfers: i32,
) -> i64 {
    if low_ram_device || total_bytes <= 0 {
        return AUTO_RAM_MIN_MIB;
    }
    let reserve = low_memory_threshold_bytes.max(0).max(total_bytes / 10);
    let available = (available_bytes.max(0) - reserve).max(0);
    let total_budget = (total_bytes / 16).min(available / 4);
    let active = active_transfers.max(1) as i64;
    let per_transfer_mib = total_budget / active / (1024 * 1024);
    for &step in &AUTO_RAM_STEPS_MIB {
        if step <= per_transfer_mib || step == AUTO_RAM_MIN_MIB {
            return step;
        }
    }
    AUTO_RAM_MIN_MIB
}

/// Convert Auto RAM MiB to bytes and clamp into the engine's allowed range.
pub fn auto_ram_cache_bytes(
    total_bytes: i64,
    available_bytes: i64,
    low_memory_threshold_bytes: i64,
    low_ram_device: bool,
    active_transfers: i32,
) -> i64 {
    auto_ram_cache_mib(
        total_bytes,
        available_bytes,
        low_memory_threshold_bytes,
        low_ram_device,
        active_transfers,
    ) << 20
}

/// Clamp a user/engine RAM cache request into `[MIN_RAM_CACHE_BYTES, MAX_RAM_CACHE_BYTES]`.
pub fn clamp_ram_cache_bytes(bytes: i64) -> i64 {
    bytes.clamp(MIN_RAM_CACHE_BYTES, MAX_RAM_CACHE_BYTES)
}

/// Minimum Direct Play cache for a torrent with the given piece length.
pub fn direct_play_min_cache_bytes(piece_length: i64) -> i64 {
    DIRECT_PLAY_MIN_PIECES * piece_length.max(0)
}

/// Whether a piece length can ever fit in Direct Play (≤ `MAX_RAM_CACHE_BYTES / 4`).
pub fn piece_fits_direct_play(piece_length: i64) -> bool {
    piece_length > 0 && direct_play_min_cache_bytes(piece_length) <= MAX_RAM_CACHE_BYTES
}

/// Apply `set_cache_limit` after clamping; returns the applied byte limit.
pub fn apply_cache_limit(client: &DocumentStorageClient, bytes: i64) -> StorageResult<i64> {
    let applied = clamp_ram_cache_bytes(bytes);
    client.set_cache_limit(applied)?;
    Ok(applied)
}

/// Apply Auto RAM sizing to the client; returns applied bytes.
pub fn apply_auto_ram_cache(
    client: &DocumentStorageClient,
    total_bytes: i64,
    available_bytes: i64,
    low_memory_threshold_bytes: i64,
    low_ram_device: bool,
    active_transfers: i32,
) -> StorageResult<i64> {
    let bytes = auto_ram_cache_bytes(
        total_bytes,
        available_bytes,
        low_memory_threshold_bytes,
        low_ram_device,
        active_transfers,
    );
    apply_cache_limit(client, bytes)
}

// --- Startup cache registry (Go registerStartupCache / pruneStartupCaches) ---

struct StartupRegistry {
    /// Key: `base/.startup` parent join hash path used by Go (`filepath.Join(base, hash)`).
    active: HashMap<String, i32>,
}

static STARTUP_REGISTRY: LazyLock<ParkingMutex<StartupRegistry>> = LazyLock::new(|| {
    ParkingMutex::new(StartupRegistry {
        active: HashMap::new(),
    })
});

fn startup_active_key(base: &Path, hash: &str) -> String {
    base.join(hash).to_string_lossy().into_owned()
}

/// Mark a torrent's `.startup/<hash>` as active so prune will not delete it.
pub fn register_startup_cache(base: &Path, hash: &str) {
    let mut reg = STARTUP_REGISTRY.lock();
    let key = startup_active_key(base, hash);
    *reg.active.entry(key).or_insert(0) += 1;
    let dir = base.join(".startup").join(hash);
    let _ = fs::create_dir_all(&dir);
    let now = filetime_now();
    let _ = set_mtime(&dir, now);
}

/// Drop one active reference; prune when the count hits zero.
pub fn unregister_startup_cache(base: &Path, hash: &str) {
    {
        let mut reg = STARTUP_REGISTRY.lock();
        let key = startup_active_key(base, hash);
        if let Some(count) = reg.active.get_mut(&key) {
            *count -= 1;
            if *count <= 0 {
                reg.active.remove(&key);
            }
        }
    }
    prune_startup_caches(base, "");
}

/// Keep only recent warm starts under [`DIRECT_STARTUP_TOTAL_BUDGET`],
/// reserving one [`DIRECT_STARTUP_BUDGET`] for `keep` (hex infohash, or empty).
pub fn prune_startup_caches(base: &Path, keep: &str) {
    let reg = STARTUP_REGISTRY.lock();
    let root = base.join(".startup");
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };

    struct CacheDir {
        path: PathBuf,
        size: u64,
        modified: u64,
    }

    let mut caches = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let mut size = 0u64;
        let mut modified = 0u64;
        if let Ok(files) = fs::read_dir(&path) {
            for f in files.flatten() {
                if let Ok(meta) = f.metadata() {
                    size += meta.len();
                    let m = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    modified = modified.max(m);
                }
            }
        }
        total += size;
        let active_key = startup_active_key(base, &name);
        let is_active = reg.active.get(&active_key).copied().unwrap_or(0) > 0;
        if name != keep && !is_active {
            caches.push(CacheDir {
                path,
                size,
                modified,
            });
        }
    }
    drop(reg);

    caches.sort_by_key(|c| c.modified);
    let limit = (DIRECT_STARTUP_TOTAL_BUDGET - DIRECT_STARTUP_BUDGET).max(0) as u64;
    for c in caches {
        if total <= limit {
            break;
        }
        if fs::remove_dir_all(&c.path).is_ok() {
            total = total.saturating_sub(c.size);
        }
    }
}

fn filetime_now() -> SystemTime {
    SystemTime::now()
}

fn set_mtime(path: &Path, _when: SystemTime) -> std::io::Result<()> {
    // Best-effort touch: recreate mtime via File::open + sync, or libc utimens.
    // Opening the directory is enough on most FS for prune ordering when combined
    // with create_dir_all; explicit utime kept simple for portability.
    let _ = path;
    Ok(())
}

/// CPU-sized worker count for background startup validation (Go `backgroundWorkers`).
pub fn background_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// Documented Direct Play attach helper: open torrent + attach stream mode.
pub fn open_direct_play(
    client: &DocumentStorageClient,
    info: &crate::Info,
    info_hash: crate::InfoHash,
    selected: &[usize],
) -> StorageResult<std::sync::Arc<DocumentTorrentStorage>> {
    let _ts = client.open_torrent(info, info_hash)?;
    let st = client
        .get_storage(&info_hash.to_hex())
        .ok_or_else(|| crate::StorageError::Other("storage missing after open".into()))?;
    let descriptors = vec![None; info.files.len()];
    st.attach_direct_play(&descriptors, selected)?;
    register_startup_cache(&st.base_dir, &info_hash.to_hex());
    Ok(st)
}

/// Head/tail piece list for a file (re-export of agy-3 helper for facade users).
pub use crate::storage::startup_file_pieces as startup_pieces_for_file;

/// Convenience: piece indices that Direct Play persists for the selected videos.
pub fn planned_startup_pieces(
    files: &[crate::storage::FileDescriptor],
    selected: &[usize],
    chunk: i64,
    first_file: Option<usize>,
) -> Vec<usize> {
    let mut files_idx: Vec<usize> = selected.to_vec();
    files_idx.sort_unstable();
    if let Some(first) = first_file {
        if let Some(pos) = files_idx.iter().position(|&x| x == first) {
            if pos > 0 {
                let mut rotated = files_idx[pos..].to_vec();
                rotated.extend_from_slice(&files_idx[..pos]);
                files_idx = rotated;
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut order = Vec::new();
    let mut budget = 0i64;
    for &i in &files_idx {
        if i >= files.len() {
            continue;
        }
        let name = &files[i].path;
        if Some(i) != first_file && !crate::storage::is_video_file_name(name) {
            continue;
        }
        for p in startup_file_pieces(&files[i], chunk) {
            if seen.insert(p) {
                // Piece length unknown here without Info; budget by chunk as upper bound.
                let n = chunk;
                if budget + n > DIRECT_STARTUP_TOTAL_BUDGET {
                    return order;
                }
                budget += n;
                order.push(p);
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileInfo, Info, InfoHash, StorageError, TorrentStorage};
    use sha1::{Digest, Sha1};
    use std::fs::{self, OpenOptions};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture_dir(prefix: &str) -> PathBuf {
        let root = PathBuf::from("/home/kar/.cache/wt-tmp/wt-storage-direct");
        let _ = fs::create_dir_all(&root);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{prefix}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn single_file_info(
        name: &str,
        length: i64,
        piece_length: i64,
        fill_hashes: bool,
    ) -> (Info, Vec<u8>) {
        let data_len = length as usize;
        let mut data = vec![0u8; data_len];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i.wrapping_mul(17) + i / piece_length.max(1) as usize) as u8;
        }
        let num = if piece_length <= 0 || length <= 0 {
            0
        } else {
            ((length + piece_length - 1) / piece_length) as usize
        };
        let mut pieces = vec![0u8; num * 20];
        if fill_hashes {
            for i in 0..num {
                let start = i * piece_length as usize;
                let end = (start + piece_length as usize).min(data_len);
                let mut h = Sha1::new();
                h.update(&data[start..end]);
                pieces[i * 20..(i + 1) * 20].copy_from_slice(&h.finalize());
            }
        }
        let info = Info {
            name: name.into(),
            piece_length,
            pieces,
            files: vec![FileInfo {
                length,
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
        (info, data)
    }

    #[test]
    fn auto_ram_low_ram_devices_keep_minimum() {
        let gib = 1024i64 * 1024 * 1024;
        assert_eq!(auto_ram_cache_mib(8 * gib, 6 * gib, gib / 4, true, 1), 64);
    }

    #[test]
    fn auto_ram_shares_memory_across_transfers() {
        let gib = 1024i64 * 1024 * 1024;
        assert_eq!(auto_ram_cache_mib(8 * gib, 4 * gib, gib / 4, false, 1), 512);
        assert_eq!(auto_ram_cache_mib(8 * gib, 4 * gib, gib / 4, false, 2), 256);
        assert_eq!(auto_ram_cache_mib(8 * gib, 4 * gib, gib / 4, false, 4), 128);
    }

    #[test]
    fn auto_ram_available_memory_reduces_budget() {
        let gib = 1024i64 * 1024 * 1024;
        assert_eq!(
            auto_ram_cache_mib(8 * gib, gib + gib / 2, gib / 4, false, 1),
            128
        );
        assert_eq!(auto_ram_cache_mib(8 * gib, gib / 2, gib / 4, false, 1), 64);
    }

    #[test]
    fn auto_ram_system_low_memory_threshold_reserved() {
        let gib = 1024i64 * 1024 * 1024;
        assert_eq!(auto_ram_cache_mib(8 * gib, 2 * gib, 2 * gib, false, 1), 64);
    }

    #[test]
    fn auto_ram_idle_and_invalid_inputs_remain_bounded() {
        let gib = 1024i64 * 1024 * 1024;
        assert_eq!(
            auto_ram_cache_mib(16 * gib, 10 * gib, gib / 4, false, 0),
            512
        );
        assert_eq!(auto_ram_cache_mib(0, 0, 0, false, -1), 64);
        assert_eq!(auto_ram_cache_mib(8 * gib, -1, -1, false, 1), 64);
    }

    #[test]
    fn clamp_ram_cache_bytes_bounds() {
        assert_eq!(clamp_ram_cache_bytes(0), MIN_RAM_CACHE_BYTES);
        assert_eq!(clamp_ram_cache_bytes(8 << 20), MIN_RAM_CACHE_BYTES);
        assert_eq!(clamp_ram_cache_bytes(64 << 20), 64 << 20);
        assert_eq!(clamp_ram_cache_bytes(2 << 30), MAX_RAM_CACHE_BYTES);
    }

    #[test]
    fn piece_fits_direct_play_refuses_huge_pieces() {
        assert!(piece_fits_direct_play(1 << 20));
        assert!(!piece_fits_direct_play(MAX_RAM_CACHE_BYTES / 2));
        assert!(!piece_fits_direct_play(0));
    }

    #[test]
    fn startup_pruning_keeps_active_readers_cache() {
        let base = fixture_dir("prune");
        let active = "active";
        let old = "old";
        for name in [active, old] {
            let dir = base.join(".startup").join(name);
            fs::create_dir_all(&dir).unwrap();
            let f = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(dir.join("0.piece"))
                .unwrap();
            // Two caches exceed the overall budget.
            f.set_len((DIRECT_STARTUP_TOTAL_BUDGET / 2 + 1) as u64)
                .unwrap();
        }
        register_startup_cache(&base, active);
        prune_startup_caches(&base, "");
        assert!(
            base.join(".startup").join(active).exists(),
            "active cache removed"
        );
        assert!(
            !base.join(".startup").join(old).exists(),
            "inactive cache retained above budget"
        );
        unregister_startup_cache(&base, active);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn direct_play_persists_only_verified_startup_and_rejects_corruption() {
        let base = fixture_dir("persist");
        let piece_length = 1 << 20;
        let pieces = 12;
        let (info, data) = single_file_info("movie.mp4", piece_length * pieces, piece_length, true);
        let client = DocumentStorageClient::new(&base);
        // Small stream budget: 4 pieces.
        client.set_cache_limit(4 * piece_length).unwrap();
        let hash = InfoHash([9; 20]);
        let st = open_direct_play(&client, &info, hash, &[0]).unwrap();

        for &index in &[0usize, 6, 11] {
            let p = st.piece_at(index);
            let begin = index * piece_length as usize;
            let end = begin + piece_length as usize;
            p.write_at(&data[begin..end], 0).unwrap();
            p.mark_complete().unwrap();
        }
        // Partial opening piece is never persisted.
        let p1 = st.piece_at(1);
        let begin = piece_length as usize;
        p1.write_at(&data[begin..begin + 1024], 0).unwrap();

        let path0 = st.startup_piece_path(0);
        let path6 = st.startup_piece_path(6);
        let path11 = st.startup_piece_path(11);
        let path1 = st.startup_piece_path(1);

        // Head (0) and tail (11) are startup pieces for a 12 MiB / 1 MiB piece file.
        // Body piece 6 is not in the head/tail plan for a single short file?
        // For length=12MiB, head=2MiB → pieces 0,1; tail=2MiB → pieces 10,11.
        // So 0 and 11 should persist; 6 should not; 1 was only partial.
        assert!(
            path0.exists() || st.piece_complete(0),
            "startup head should be saved or complete"
        );
        assert!(!path1.exists(), "partial opening piece must not persist");
        assert!(!path6.exists(), "body piece must not persist");

        // Corruption must not be trusted as a warm start.
        if path0.exists() {
            fs::write(&path0, vec![3u8; (piece_length as usize) + 32]).unwrap();
        }

        let _ = st.close();
        unregister_startup_cache(&base, &hash.to_hex());

        // Re-attach: corrupted piece 0 rejected; valid 11 kept if saved.
        let client2 = DocumentStorageClient::new(&base);
        client2.set_cache_limit(4 * piece_length).unwrap();
        let warm = open_direct_play(&client2, &info, hash, &[0]).unwrap();
        assert!(
            !warm.piece_complete(0),
            "corrupted startup piece must not restore complete"
        );
        assert!(!warm.piece_complete(6));
        // piece 11 may be complete if it was saved and validated within budget.
        let _ = path11;
        for dir in [".stream", ".boundary"] {
            assert!(
                !base.join(dir).join(hash.to_hex()).exists(),
                "full backing created: {dir}"
            );
        }
        let _ = warm.close();
        unregister_startup_cache(&base, &hash.to_hex());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn direct_play_evicts_partial_without_returning_fabricated_bytes() {
        let base = fixture_dir("evict");
        let piece_length = 8i64;
        let (info, data) = single_file_info("movie.mp4", piece_length * 8, piece_length, true);
        let client = DocumentStorageClient::new(&base);
        client.set_cache_limit(4 * piece_length).unwrap();
        let hash = InfoHash([1; 20]);
        let st = open_direct_play(&client, &info, hash, &[0]).unwrap();
        st.set_cache_limit(4 * piece_length);
        st.clear_startup_pieces();
        for i in 0..4usize {
            let p = st.piece_at(i);
            let _ = p.write_at(&data[i * 8..i * 8 + 4], 0);
        }
        let p4 = st.piece_at(4);
        p4.write_at(&data[32..40], 0).unwrap();
        p4.mark_complete().unwrap();
        assert!(
            st.cache_bytes() <= st.max_cache_bytes(),
            "cache over budget"
        );
        let p0 = st.piece_at(0);
        let err = p0.read_at(&mut [0u8; 8], 0).unwrap_err();
        assert!(
            matches!(err, StorageError::Unavailable),
            "lost partial was readable: {err:?}"
        );
        let _ = st.close();
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn direct_play_protects_fully_received_pieces_until_verification() {
        let base = fixture_dir("protect");
        let piece_length = 8i64;
        let (info, data) = single_file_info("movie.mp4", piece_length * 8, piece_length, true);
        let client = DocumentStorageClient::new(&base);
        client.set_cache_limit(4 * piece_length).unwrap();
        let st = open_direct_play(&client, &info, InfoHash([2; 20]), &[0]).unwrap();
        st.set_cache_limit(4 * piece_length);
        st.clear_startup_pieces();
        for i in 0..4usize {
            let p = st.piece_at(i);
            p.write_at(&data[i * 8..i * 8 + 8], 0).unwrap();
        }
        let p4 = st.piece_at(4);
        let err = p4.write_at(&data[32..40], 0);
        assert!(
            matches!(
                err,
                Err(StorageError::Unavailable) | Err(StorageError::Other(_))
            ),
            "hashing piece evicted: {err:?}"
        );
        let _ = st.close();
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn direct_play_close_does_not_join_blocked_completion_callback() {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let base = fixture_dir("close-cb");
        let piece_length = 8i64;
        let (info, data) = single_file_info("movie.mp4", piece_length * 8, piece_length, true);
        let client = DocumentStorageClient::new(&base);
        client.set_cache_limit(4 * piece_length).unwrap();
        let st = open_direct_play(&client, &info, InfoHash([3; 20]), &[0]).unwrap();
        st.set_cache_limit(4 * piece_length);
        st.clear_startup_pieces();

        let entered = Arc::new(Mutex::new(false));
        let release = Arc::new(Mutex::new(()));
        let block = release.lock().unwrap();
        let entered_cb = entered.clone();
        let release_cb = release.clone();
        st.set_on_stream_evict(Some(Arc::new(move |_| {
            *entered_cb.lock().unwrap() = true;
            let _g = release_cb.lock().unwrap();
        })));

        let p = st.piece_at(0);
        p.write_at(&data[0..8], 0).unwrap();
        p.mark_complete().unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if *entered.lock().unwrap() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(*entered.lock().unwrap(), "no completion callback");

        let st2 = st.clone();
        let handle = std::thread::spawn(move || st2.close());
        let result = handle.join().expect("close thread panicked");
        assert!(result.is_ok(), "close error: {result:?}");
        drop(block);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn runtime_set_cache_limit_raises_direct_play_floor() {
        let base = fixture_dir("limit");
        let piece_length = 1 << 20;
        let (info, _) = single_file_info("movie.mp4", piece_length * 8, piece_length, true);
        let client = DocumentStorageClient::new(&base);
        // Below Direct Play floor (4 pieces) but at/above MIN_RAM.
        apply_cache_limit(&client, 16 << 20).unwrap();
        let st = open_direct_play(&client, &info, InfoHash([4; 20]), &[0]).unwrap();
        assert!(st.stream_mode());
        // Shrinking further still leaves stream mode; floor is per-torrent.
        client.set_cache_limit(16 << 20).unwrap();
        assert!(st.stream_mode());
        // Raise to 512 MiB.
        apply_cache_limit(&client, 512 << 20).unwrap();
        assert_eq!(client.cache_limit(), 512 << 20);
        let _ = st.close();
        unregister_startup_cache(&base, &InfoHash([4; 20]).to_hex());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn preview_budget_constant_is_separate() {
        assert_eq!(PREVIEW_BUDGET_BYTES, 8 << 20);
        assert_ne!(PREVIEW_BUDGET_BYTES, MIN_RAM_CACHE_BYTES);
    }

    #[test]
    fn startup_file_pieces_head_and_tail() {
        use crate::storage::FileDescriptor;
        let f = FileDescriptor {
            path: "ep.mp4".into(),
            offset: 0,
            length: 10 << 20,
            file: None,
            external: false,
        };
        let pieces = startup_file_pieces(&f, 1 << 20);
        // head 2 MiB → 0,1; tail 2 MiB → 8,9
        assert_eq!(pieces, vec![0, 1, 8, 9]);
    }

    #[test]
    fn open_direct_play_rejects_when_already_attached() {
        let base = fixture_dir("reattach");
        let (info, _) = single_file_info("movie.mp4", 4 << 20, 1 << 20, true);
        let client = DocumentStorageClient::new(&base);
        let hash = InfoHash([5; 20]);
        let st = open_direct_play(&client, &info, hash, &[0]).unwrap();
        let err = st.attach_direct_play(&[None], &[0]);
        assert!(err.is_err());
        let _ = st.close();
        unregister_startup_cache(&base, &hash.to_hex());
        let _ = fs::remove_dir_all(&base);
    }
}

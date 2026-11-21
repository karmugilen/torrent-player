//! One-keyframe thumbnails, for every torrent from the moment metadata arrives.
//!
//! ```text
//!  metadata -> fetch container index (head/tail, moov/cues)
//!           -> pick the keyframe nearest 27% of the video
//!           -> fetch only that keyframe's piece(s)
//!           -> publish its time on /preview-state (keyframeUs)
//! ```
//!
//! The app decodes exactly that time, so the decoder only reads bytes that are
//! already here. Before configure the bytes sit in the preview RAM cache; in
//! Direct Play the keyframe piece also joins the on-disk startup cache.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use wt_core::PiecePriority;

use crate::playback::PlaybackHost;
use crate::seek_index::{thumb_keyframe, video_index, KEYFRAME_FETCH_BYTES};
use crate::server::{EngineServer, TorrentRecord};

/// Where in the video the thumbnail comes from.
const THUMB_FRACTION: f64 = 0.27;
const POLL: Duration = Duration::from_millis(300);
const MAX_RUN: Duration = Duration::from_secs(15 * 60);
/// About every 2 s (passes are 300 ms apart).
const WAKE_EVERY_PASSES: usize = 7;
static WAKE_TICK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
/// While the index is unknown, fetch the file's ends too: moov / cues live there.
const HEAD_BYTES: i64 = 2 << 20;
const TAIL_BYTES: i64 = 4 << 20;

/// Pieces of a torrent covering a keyframe that starts at `abs`.
pub fn thumb_pieces(abs: i64, piece_length: i64, file_start: i64, file_end: i64) -> Vec<usize> {
    if piece_length <= 0 || abs < file_start || abs >= file_end {
        return Vec::new();
    }
    let end = (abs + KEYFRAME_FETCH_BYTES).min(file_end);
    ((abs / piece_length) as usize..=((end - 1) / piece_length) as usize).collect()
}

/// Pieces covering torrent bytes `[start, end)`.
fn span_pieces(start: i64, end: i64, piece_length: i64) -> impl Iterator<Item = u32> {
    let (first, last) = if piece_length <= 0 || end <= start {
        (1, 0)
    } else {
        (start / piece_length, (end - 1) / piece_length)
    };
    (first..=last).map(|p| p as u32)
}

/// The videos to plan: the selection once configured, else the largest video.
pub fn thumb_targets(files: &[(usize, String, i64)], selected: &[usize], configured: bool) -> Vec<usize> {
    let videos = files.iter().filter(|(_, name, len)| *len > 0 && wt_storage::is_video_file_name(name));
    if configured {
        videos.filter(|(i, _, _)| selected.contains(i)).map(|(i, _, _)| *i).collect()
    } else {
        videos.max_by_key(|(_, _, len)| *len).map(|(i, _, _)| vec![*i]).unwrap_or_default()
    }
}

fn file_list(rec: &TorrentRecord) -> Vec<(usize, String, i64)> {
    rec.torrent.files().into_iter().map(|f| (f.index(), f.path(), f.length())).collect()
}

#[derive(Default)]
struct FilePlan {
    /// (byte offset in file, seconds) of the chosen keyframe.
    found: Option<(i64, f64)>,
    unindexed: bool,
    /// Storage mode the startup cache was told about, `(configured, direct_play)`.
    cached_for: Option<(bool, bool)>,
    /// Pieces holding the container index (MP4 moov, MKV head + Cues): what
    /// a player reads before it knows the video's times. Kept with the thumbnail.
    index_pieces: Vec<usize>,
}

impl EngineServer {
    /// Start the thumbnail planner of a torrent, unless it already runs.
    pub(crate) fn spawn_thumb_keyframes(&self, id: String) {
        let Some(record) = self.record_by_id(&id) else { return };
        let flag = record.read().thumb_planner.clone();
        if flag.swap(true, Ordering::SeqCst) {
            return; // running; it notices configure changes itself
        }
        let weak = self.self_ref.get().cloned().unwrap_or_default();
        let thread_flag = flag.clone();
        let spawned = std::thread::Builder::new().name("wt-thumb-keyframe".into()).spawn(move || {
            let mut plans: HashMap<usize, FilePlan> = HashMap::new();
            let started = std::time::Instant::now();
            while let Some(done) = plan_once(&weak, &id, &mut plans) {
                if done || started.elapsed() > MAX_RUN {
                    break;
                }
                std::thread::sleep(POLL);
            }
            if let Some(server) = weak.upgrade() {
                if let Some(record) = server.record_by_id(&id) {
                    let coordinator = record.read().playback_coordinator.clone();
                    coordinator.set_thumb_demand(HashMap::new());
                    server.publish_demand(&id, &coordinator.effective());
                }
            }
            thread_flag.store(false, Ordering::SeqCst);
            // A configure that landed while this thread was ending found it running.
            if let Some(server) = weak.upgrade() {
                if let Some(record) = server.record_by_id(&id) {
                    let pending = {
                        let r = record.read();
                        !r.torrent.closed() && r.torrent.info().is_some() && !r.paused
                            && thumb_targets(&file_list(&r), &r.selected, r.configured).iter()
                                .any(|i| !r.thumb_keyframes.contains_key(i) && !r.thumb_unindexed.contains(i))
                    };
                    if pending && started.elapsed() <= MAX_RUN {
                        server.spawn_thumb_keyframes(id);
                    }
                }
            }
        });
        if spawned.is_err() {
            flag.store(false, Ordering::SeqCst);
        }
    }
}

/// One planning pass. `None` ends the planner (torrent gone); else whether
/// every target has its thumbnail keyframe (or has none to find).
fn plan_once(weak: &std::sync::Weak<EngineServer>, id: &str, plans: &mut HashMap<usize, FilePlan>) -> Option<bool> {
    let server = weak.upgrade()?;
    let record = server.record_by_id(id)?;
    let (files, targets, mode, piece_length, hash, coordinator, playing) = {
        let rec = record.read();
        if rec.torrent.closed() {
            return None;
        }
        // Not configured and not a draft: a restore or seed whose configure is
        // moments away. Wait for it rather than borrow preview storage.
        if !rec.configured && !rec.prepared {
            return Some(false);
        }
        if rec.torrent.info().is_none() || rec.paused || rec.checking {
            return Some(false);
        }
        let files: HashMap<usize, (i64, i64)> = rec.torrent.files().into_iter()
            .map(|f| (f.index(), (f.offset(), f.length()))).collect();
        let targets = thumb_targets(&file_list(&rec), &rec.selected, rec.configured);
        // The record's piece_length is only filled at configure; a magnet draft has metadata already.
        let piece_length = rec.torrent.info().map(|i| i.piece_length).unwrap_or(rec.piece_length);
        (files, targets, (rec.configured, rec.direct_play), piece_length,
            rec.info_hash.clone(), rec.playback_coordinator.clone(), rec.playback_readers > 0)
    };
    let (configured, direct_play) = mode;
    if targets.is_empty() {
        return Some(true);
    }
    let storage = server.storage.get_storage(&hash)?;
    if !configured {
        // Draft (Add screen): fetch into the preview RAM cache.
        if storage.attach_preview_temp(targets[0]).is_err() {
            return Some(false);
        }
        if server.payload_transfers_allowed.load(Ordering::SeqCst) {
            record.read().torrent.allow_data_download();
        }
    }
    // Never outrank a playing video.
    let priority = if playing { PiecePriority::High } else { PiecePriority::Now };
    let mut demand: HashMap<u32, PiecePriority> = HashMap::new();
    let mut ready = Vec::new();
    let mut all_done = true;
    for &index in &targets {
        let Some(&(offset, length)) = files.get(&index) else { continue };
        let plan = plans.entry(index).or_default();
        if configured && direct_play {
            storage.load_startup_file(index);
        }
        if plan.found.is_none() && !plan.unindexed {
            let miss = Cell::new(None);
            let reads = RefCell::new(Vec::new());
            let read = |abs: i64, len: i64| {
                let mut got = storage.copy_verified_range(offset + abs, len);
                // After a restart the index may be in the startup cache on disk.
                if got.is_none() && configured && direct_play
                    && storage.load_saved_range(offset + abs, offset + abs + len)
                {
                    got = storage.copy_verified_range(offset + abs, len);
                }
                if got.is_some() {
                    reads.borrow_mut().push((offset + abs, offset + abs + len));
                } else if miss.get().is_none() {
                    miss.set(Some((abs, len)));
                }
                got
            };
            match video_index(&read, length) {
                Some(found) => {
                    let mut index_pieces: Vec<usize> = reads.borrow().iter()
                        .flat_map(|&(start, end)| span_pieces(start, end, piece_length))
                        .map(|p| p as usize)
                        .collect();
                    index_pieces.sort_unstable();
                    index_pieces.dedup();
                    plan.index_pieces = index_pieces;
                    match thumb_keyframe(&found, THUMB_FRACTION) {
                        Some(at) => plan.found = Some(at),
                        None => plan.unindexed = true,
                    }
                }
                None => {
                    all_done = false;
                    let ends = [
                        (offset, offset + HEAD_BYTES.min(length)),
                        (offset + (length - TAIL_BYTES).max(0), offset + length),
                    ];
                    let missing = miss.get().map(|(abs, len)| (offset + abs, offset + abs + len));
                    for (start, end) in ends.into_iter().chain(missing) {
                        for p in span_pieces(start, end, piece_length) {
                            if !storage.piece_complete(p as usize) {
                                demand.insert(p, priority);
                            }
                        }
                    }
                    continue;
                }
            }
        }
        if plan.unindexed {
            if configured && direct_play && plan.cached_for != Some(mode) {
                storage.set_thumb_pieces(index, plan.index_pieces.clone());
                plan.cached_for = Some(mode);
            }
            ready.push((index, None));
            continue;
        }
        let Some((at, secs)) = plan.found else { continue };
        let pieces = thumb_pieces(offset + at, piece_length, offset, offset + length);
        if plan.cached_for != Some(mode) {
            wt_types::events::record("thumb_keyframe", format!(
                "file={index} at={at} secs={secs:.2} pieces={pieces:?} index={:?} configured={configured} dp={direct_play}",
                plan.index_pieces));
            if configured && direct_play {
                let mut keep = plan.index_pieces.clone();
                keep.extend(pieces.iter().copied());
                keep.sort_unstable();
                keep.dedup();
                storage.set_thumb_pieces(index, keep);
            }
            plan.cached_for = Some(mode);
        }
        let missing: Vec<usize> = pieces.iter().copied().filter(|&p| !storage.piece_complete(p)).collect();
        if missing.is_empty() {
            ready.push((index, Some(secs)));
        } else {
            all_done = false;
            demand.extend(missing.into_iter().map(|p| (p as u32, priority)));
        }
    }
    let waiting = !demand.is_empty();
    coordinator.set_thumb_demand(demand);
    server.publish_demand(id, &coordinator.effective());
    // Unchanged priorities wake nobody: after a configure an idle peer can sit
    // unasked forever. Wake peers now and then while the thumbnail waits.
    if waiting && WAKE_TICK.fetch_add(1, Ordering::Relaxed) % WAKE_EVERY_PASSES == 0 {
        record.read().torrent.bump_change();
    }
    if !ready.is_empty() {
        let mut rec = record.write();
        // A configure during this pass replaced the storage: check again next pass.
        if (rec.configured, rec.direct_play) == mode {
            let mut changed = false;
            for (index, secs) in ready {
                changed |= match secs {
                    Some(secs) => rec.thumb_keyframes.insert(index, secs).is_none(),
                    None => rec.thumb_unindexed.insert(index),
                };
            }
            if changed {
                rec.torrent.update_all_piece_completion();
                if configured && direct_play {
                    server.refresh_startup_hints(&mut rec);
                }
                drop(rec);
                server.notify_change();
            }
        } else {
            all_done = false;
        }
    }
    if all_done && !configured && server.preview_active.lock().is_none() {
        record.read().torrent.disallow_data_download();
    }
    Some(all_done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_pieces_stay_inside_the_file() {
        let mib = 1 << 20;
        // 512 KiB from 1.75 MiB crosses into piece 2.
        assert_eq!(thumb_pieces(7 * mib / 4, mib, 0, 10 * mib), vec![1, 2]);
        // Near the file end it stops at the last piece of the file.
        assert_eq!(thumb_pieces(10 * mib - 10, mib, 0, 10 * mib), vec![9]);
        assert!(thumb_pieces(-1, mib, 0, 10 * mib).is_empty());
        assert!(thumb_pieces(5, 0, 0, 10).is_empty());
    }

    #[test]
    fn draft_plans_the_largest_video_configured_plans_the_selection() {
        let files = vec![
            (0, "a/sample.mkv".to_string(), 10),
            (1, "a/movie.mp4".to_string(), 900),
            (2, "a/subs.srt".to_string(), 5000),
            (3, "a/extra.mkv".to_string(), 50),
        ];
        assert_eq!(thumb_targets(&files, &[], false), vec![1]);
        assert_eq!(thumb_targets(&files, &[0, 2, 3], true), vec![0, 3]);
        assert!(thumb_targets(&files, &[], true).is_empty());
    }

    #[test]
    fn span_pieces_cover_the_range() {
        assert_eq!(span_pieces(5, 25, 10).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(span_pieces(10, 20, 10).collect::<Vec<_>>(), vec![1]);
        assert!(span_pieces(5, 5, 10).next().is_none());
    }
}

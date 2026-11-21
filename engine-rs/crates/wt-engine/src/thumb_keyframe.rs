//! Direct Play thumbnails from the startup cache.
//!
//! Head and tail of each video are already kept on disk (startup cache).
//! They hold the container index, so once they arrive we pick one keyframe
//! near a quarter of the video and keep its piece(s) in that cache too.
//! A cached-only preview then finds every byte it needs, and the app asks
//! for the frame at exactly that keyframe's time (`/preview-state`).

use std::sync::Arc;
use std::time::Duration;

use crate::seek_index::{thumb_keyframe, video_index, KEYFRAME_FETCH_BYTES};
use crate::server::EngineServer;

/// Where in the video the thumbnail comes from; the app picks 18-36%.
const THUMB_FRACTION: f64 = 0.27;
const POLL: Duration = Duration::from_secs(2);
const MAX_POLLS: usize = 450; // 15 minutes

/// Pieces of a torrent covering a keyframe that starts at `abs`.
pub fn thumb_pieces(abs: i64, piece_length: i64, file_start: i64, file_end: i64) -> Vec<usize> {
    if piece_length <= 0 || abs < file_start || abs >= file_end {
        return Vec::new();
    }
    let end = (abs + KEYFRAME_FETCH_BYTES).min(file_end);
    ((abs / piece_length) as usize..=((end - 1) / piece_length) as usize).collect()
}

impl EngineServer {
    /// Start planning thumbnail keyframes for a Direct Play torrent.
    pub(crate) fn spawn_thumb_keyframes(&self, id: String, storage: Arc<wt_storage::DocumentTorrentStorage>) {
        let weak = self.self_ref.get().cloned().unwrap_or_default();
        let _ = std::thread::Builder::new().name("wt-thumb-keyframe".into()).spawn(move || {
            let mut done = std::collections::HashSet::new();
            for _ in 0..MAX_POLLS {
                let Some(server) = weak.upgrade() else { return };
                let Some(record) = server.record_by_id(&id) else { return };
                let (files, piece_length) = {
                    let rec = record.read();
                    if rec.torrent.closed() || !rec.direct_play {
                        return;
                    }
                    let files: Vec<_> = rec.torrent.files().into_iter()
                        .filter(|f| rec.selected.contains(&f.index())
                            && wt_storage::is_video_file_name(&f.path())
                            && !done.contains(&f.index()))
                        .map(|f| (f.index(), f.offset(), f.length()))
                        .collect();
                    (files, rec.piece_length)
                };
                if files.is_empty() {
                    return;
                }
                for (index, offset, length) in files {
                    storage.load_startup_file(index);
                    let read = |abs: i64, len: i64| storage.copy_verified_range(offset + abs, len);
                    let Some(found) = video_index(&read, length) else { continue };
                    done.insert(index);
                    let Some((at, secs)) = thumb_keyframe(&found, THUMB_FRACTION) else { continue };
                    let pieces = thumb_pieces(offset + at, piece_length, offset, offset + length);
                    wt_types::events::record("thumb_keyframe", format!("file={index} at={at} secs={secs:.2} pieces={pieces:?}"));
                    storage.set_thumb_pieces(index, pieces);
                    let mut rec = record.write();
                    rec.thumb_keyframes.insert(index, secs);
                    rec.torrent.update_all_piece_completion();
                    server.refresh_startup_hints(&mut rec);
                }
                drop(server);
                std::thread::sleep(POLL);
            }
        });
    }
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
}

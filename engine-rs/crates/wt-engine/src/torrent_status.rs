//! Verified completion, received progress and per-torrent transfer rates.
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::{EngineServer, TorrentRecord};
use crate::types::FileView;

impl TorrentRecord {
    pub(super) fn refresh_status(&mut self) {
        let Some(info) = self.torrent.info() else {
            return;
        };
        let piece_length = info.piece_length;
        if piece_length <= 0 {
            return;
        }
        self.name = Some(info.name.clone());
        self.piece_length = piece_length;
        let files = self.torrent.files();
        let pieces = self.torrent.inner.pieces.read();
        let verified: Vec<bool> = pieces.iter().map(|p| p.complete).collect();
        self.total_pieces = pieces.len();
        self.last_piece_length = pieces.last().map_or(0, |p| info.piece(p.index).length);
        let changed = verified != self.piece_verified || files.len() != self.files.len();
        if files.len() != self.files.len() {
            self.files = files
                .iter()
                .map(|file| {
                    let path = file.path();
                    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                    FileView {
                        index: file.index(),
                        length: file.length(),
                        file_type: crate::stream_http::content_type_for_name(&name).to_owned(),
                        name,
                        path,
                        progress: 0.0,
                        verified_bytes: 0,
                    }
                })
                .collect();
        }
        self.downloaded = 0;
        self.verified_bytes = 0;
        for (view, file) in self.files.iter_mut().zip(files.iter()) {
            let start = file.offset();
            let end = start + file.length();
            let mut received = 0;
            let mut file_verified = 0;
            for i in
                (start / piece_length) as usize..((end + piece_length - 1) / piece_length) as usize
            {
                let Some(piece) = pieces.get(i) else { break };
                let piece_start = i as i64 * piece_length;
                let piece_end = (piece_start + piece_length).min(info.total_length());
                let overlap = (end.min(piece_end) - start.max(piece_start)).max(0);
                if piece.complete {
                    received += overlap;
                    if changed {
                        file_verified += overlap;
                    }
                } else {
                    for &offset in &piece.chunks_received {
                        let block_start = piece_start + i64::from(offset);
                        let block_end =
                            (block_start + i64::from(wt_types::BLOCK_SIZE)).min(piece_end);
                        received += (end.min(block_end) - start.max(block_start)).max(0);
                    }
                }
            }
            if changed {
                view.verified_bytes = file_verified;
            }
            received = received.max(view.verified_bytes).min(view.length);
            view.progress = if view.length > 0 {
                received as f64 / view.length as f64
            } else {
                1.0
            };
            self.downloaded += received;
            self.verified_bytes += view.verified_bytes;
        }
        self.piece_verified = verified;
        drop(pieces);
        let stats = self.torrent.stats();
        self.uploaded = stats.conn_stats.bytes_written_data;
        self.rates.sample(
            Instant::now(),
            stats.conn_stats.bytes_read_data,
            stats.conn_stats.bytes_written_data,
            self.paused || self.torrent.closed(),
        );
    }
}

impl EngineServer {
    /// Uses each torrent's counters; client-wide rates belong only in /stats.
    pub(crate) fn sample_transfer_rates(&self, now: Instant) {
        let records: Vec<_> = self.records.read().values().cloned().collect();
        let (mut down, mut up) = (0, 0);
        for record in records {
            let mut rec = record.write();
            let stats = rec.torrent.stats();
            let paused = rec.paused || rec.torrent.closed();
            rec.rates.sample(
                now,
                stats.conn_stats.bytes_read_data,
                stats.conn_stats.bytes_written_data,
                paused,
            );
            down += rec.rates.down;
            up += rec.rates.up;
        }
        self.download_speed.store(down, Ordering::SeqCst);
        self.upload_speed.store(up, Ordering::SeqCst);
    }
}

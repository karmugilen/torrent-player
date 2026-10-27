//! Cheap status change detection and bounded piece/playback telemetry.
use crate::server::EngineServer;
use crate::types::{PieceBucket, PieceTelemetryResponse, PlaybackDetail, PlaybackPieceRange};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};
use wt_policy::playback_grid::{playback_detail, playback_focus_window, playback_tiers};
use wt_policy::PlaybackWindow;
use wt_storage::pieces::{aggregate_piece_buckets, compress_piece_runs, PieceRange, PieceStateRun};

impl EngineServer {
    pub fn playback_windows(&self, id: &str) -> Vec<PlaybackWindow> {
        // Release the registry lock before taking session locks: reads can call the host.
        let readers: Vec<_> = self
            .playback_readers
            .lock()
            .values()
            .filter(|r| r.id == id && !r.is_closed())
            .cloned()
            .collect();
        readers
            .into_iter()
            .filter_map(|r| r.window_snapshot())
            .collect()
    }

    pub(crate) fn telemetry_fingerprint(&self) -> [u8; 32] {
        let mut records: Vec<_> = self.records.read().values().cloned().collect();
        records.sort_by_key(|r| r.read().id.clone());
        let mut h = Sha256::new();
        h.update(format!(
            "{}/{};",
            self.download_speed.load(Ordering::Relaxed),
            self.upload_speed.load(Ordering::Relaxed)
        ));
        for record in records {
            let rec = record.read();
            let stats = rec.torrent.stats();
            // No piece-state run walk: changing priority alone must not wake Android.
            h.update(format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{};",
                rec.id,
                rec.paused,
                rec.checking,
                rec.configured,
                rec.checked_pieces,
                rec.check_total,
                rec.selected.len(),
                stats.active_peers,
                stats.pieces_complete,
                rec.torrent.bytes_completed(),
                stats.conn_stats.bytes_read_data,
                stats.conn_stats.bytes_written_data,
                rec.downloaded,
                rec.uploaded,
                rec.error.as_deref().unwrap_or("")
            ));
            if rec.direct_play {
                h.update(format!("idle:{}:{};", rec.resting, rec.idle_drop_ready));
                if let Some(st) = self.storage.get_storage(&rec.info_hash) {
                    h.update(st.stream_revision().to_le_bytes());
                }
            }
            if let Some(np) = &rec.next_preload {
                h.update(format!(
                    "next:{}:{}:{};",
                    np.file_index, np.active, np.ready
                ));
            }
        }
        h.finalize().into()
    }

    pub(crate) fn piece_telemetry(&self, id: &str, query: &str) -> (u16, Vec<u8>) {
        let Some(record) = self.records.read().get(id).cloned() else {
            return (
                404,
                serde_json::to_vec(&serde_json::json!({"error":"torrent not found"})).unwrap(),
            );
        };
        let windows = self.playback_windows(id);
        let mut rec = record.write();
        rec.refresh();
        let max_buckets = url::form_urlencoded::parse(query.as_bytes())
            .find_map(|(k, v)| {
                (k == "maxBuckets")
                    .then(|| v.parse::<usize>().ok())
                    .flatten()
                    .filter(|n| *n > 0)
            })
            .unwrap_or(256)
            .min(1024);
        let total = rec.torrent.num_pieces();
        let storage = rec
            .direct_play
            .then(|| self.storage.get_storage(&rec.info_hash))
            .flatten();
        let runs = if let Some(st) = &storage {
            st.stream_piece_runs(total as usize)
        } else {
            let runs = rec
                .torrent
                .piece_state_runs()
                .into_iter()
                .map(|r| PieceStateRun {
                    length: r.length as usize,
                    complete: r.state.complete,
                    checking: r.state.checking,
                    hashing: false,
                    partial: r.state.partial,
                    priority: r.state.priority,
                })
                .collect::<Vec<_>>();
            compress_piece_runs(&runs, total as usize, !windows.is_empty())
        };
        let ranges = rec
            .torrent
            .files()
            .into_iter()
            .filter(|f| rec.selected.contains(&f.index()) && f.length() > 0)
            .map(|f| PieceRange {
                start: (f.offset() / rec.piece_length.max(1)) as u32,
                end: ((f.offset() + f.length() + rec.piece_length.max(1) - 1)
                    / rec.piece_length.max(1)) as u32,
            })
            .collect::<Vec<_>>();
        let buckets = runs
            .and_then(|runs| aggregate_piece_buckets(&runs, &ranges, total, max_buckets as u32));
        let buckets = match buckets {
            Ok(b) => b,
            Err(e) => {
                return (
                    500,
                    serde_json::to_vec(&serde_json::json!({"error":e.to_string()})).unwrap(),
                )
            }
        };
        let mut response = PieceTelemetryResponse {
            id: rec.id.clone(),
            info_hash: Some(rec.info_hash.clone()),
            generation: 1,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            total_pieces: total as usize,
            piece_length: rec.piece_length,
            last_piece_length: rec.last_piece_length,
            max_buckets,
            buckets: buckets
                .into_iter()
                .map(|b| PieceBucket {
                    start: b.start as usize,
                    end: b.end as usize,
                    total: b.total as usize,
                    selected: b.selected as usize,
                    verified: b.verified as usize,
                    receiving: b.receiving as usize,
                    selected_verified: b.selected_verified as usize,
                    selected_receiving: b.selected_receiving as usize,
                })
                .collect(),
            ghost_pro_mode: self.ghost_pro_mode.load(Ordering::SeqCst),
            playback: None,
            playheads: None,
            playback_detail: None,
        };
        if !rec.paused
            && !rec.checking
            && rec.error.is_none()
            && self.payload_transfers_allowed.load(Ordering::SeqCst)
        {
            let mut ranges = Vec::new();
            let mut heads = Vec::new();
            for window in &windows {
                let (tiers, head) = playback_tiers(window, rec.piece_length);
                ranges.extend(tiers.into_iter().map(|r| PlaybackPieceRange {
                    start: r.start as usize,
                    end: r.end as usize,
                    priority: r.priority,
                }));
                if let Some(head) = head {
                    heads.push(head as usize);
                }
            }
            response.playback = (!ranges.is_empty()).then_some(ranges);
            response.playheads = (!heads.is_empty()).then_some(heads);
            if let Some(window) = playback_focus_window(None, &windows) {
                let span = wt_policy::piece_span::union_span(&[
                    window.urgent,
                    window.readahead,
                    window.lookahead,
                ]);
                let states = storage.as_ref().map(|st| {
                    st.piece_grid_states(
                        span.begin as usize,
                        span.end.min(span.begin.saturating_add(2048)).min(total) as usize,
                    )
                });
                response.playback_detail = playback_detail(window, total, |p| {
                    if let Some(states) = &states {
                        return states[(p - span.begin) as usize];
                    }
                    let s = rec.torrent.piece_state(p);
                    if s.complete {
                        b'v'
                    } else if s.partial || s.checking {
                        b'r'
                    } else {
                        b'm'
                    }
                })
                .map(|d| PlaybackDetail {
                    start: d.start as usize,
                    states: d.states,
                    playhead: d.playhead as usize,
                });
            }
        }
        (200, serde_json::to_vec(&response).unwrap())
    }
}

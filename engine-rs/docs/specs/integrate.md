# Task: wire EngineServer onto wt_core / wt_storage / wt_policy (integration)
Target: engine-rs/crates/wt-engine/src/server.rs, types.rs, lib.rs, trackers.rs, tracker_status.rs, seeding.rs, connections.rs (yours). NOT yours: playback.rs, stream_http.rs, preview.rs, live_preview.rs, network.rs (grok-3); src/jni, src/bin, ctl_http.rs (grok-4).
Why: your previous task is settled, but server.rs does not use wt_core, wt_storage or wt_policy at all (grep finds 0 uses), so the engine cannot download, seed or stream. That is the single most important remaining piece of the port.
Change, mirroring engine-go/server.go:
1. EngineServer owns one wt_core::Client built with wt_core::ClientConfig set exactly as server.go lines 190-320, with default_storage = wt_storage::DocumentStorageClient for downloads and wt_storage::direct_storage (grok-5's facade: open_direct_play, Auto RAM, cache limits) for Direct Play.
2. TorrentRecord wraps a wt_core::Torrent. Routes: /add (wt_meta::parse_magnet / parse_metainfo -> TorrentSpec -> Client::add_torrent_spec), /torrent/<id> and /stats (Torrent::stats, files, bytes_completed, peer_conns, piece_state_runs, info), /select, /pause, /resume, /remove (+ ClientStorage::remove_torrent), /configure, /forget-peers, /pieces, /metadata, /settings (apply to Client and storages), tracker status from ClientConfig.callbacks.status_updated, seeding policy via allow/disallow_data_upload.
3. Implement grok-3's PlaybackHost trait (below) for EngineServer over wt_core::Torrent/File/Reader, and delegate open/read/close playback and preview to grok-3's PlaybackSession, keeping the frozen EngineServer signatures.
4. Use wt_policy wherever server.go uses the corresponding Go policy (transfer rates, peer exploration, ghost modes, bypass hook via Client::set_outgoing_tcp_conn_hook).
Acceptance (all must hold, report each honestly):
- grep shows wt_core::Client and wt_core::Torrent used in server.rs;
- cargo build -p wt-engine --bin engine-host succeeds, and two engine-host processes on localhost transfer a small fixture torrent: one seeds a completed file, the other adds it as a magnet with x.pe=127.0.0.1:<port> and reaches 100% (show the /torrent/<id> JSON before and after);
- un-ignore and pass engine_server, engine_control, engine_settings, engine_telemetry tests as far as the core allows; list the rest with reasons.
If a wt-core API is missing or broken, ask the coordinator (it is routed to agy-2) and keep going on the other routes.

## grok-3 PlaybackHost trait (implement this)
Please forward to agy-4. playback.rs will define:

pub trait ReaderBackend: Send {
  fn read_at(&mut self, buf: &mut [u8], offset: i64) -> io::Result<usize>;
  fn set_readahead(&mut self, bytes: i64);
  fn set_readahead_fn(&mut self, f: Option<ReadaheadCallback>);
  fn set_responsive(&mut self, on: bool);
  fn cancel(&mut self);
  fn close(&mut self);
}

pub struct PlaybackFileInfo { index, name, length, torrent_offset, begin_piece, end_piece, verified }
pub struct PlaybackTorrentSnapshot { configured, direct_play, file_only, selected, focused_file, files: Vec<PlaybackFileInfo>, piece_length, open_reader_count }

pub trait PlaybackHost: Send + Sync {
  fn torrent_for_playback(&self, id: &str) -> Option<PlaybackTorrentSnapshot>;
  fn is_shutting_down(&self) -> bool;
  fn payload_allowed(&self) -> bool;
  fn ghost_pro(&self) -> bool;
  fn open_file_reader(&self, id: &str, file_index: usize) -> io::Result<Box<dyn ReaderBackend>>;
  fn apply_piece_priorities(&self, id: &str, demand: &HashMap<u32, PiecePriority>);
  fn clear_piece_priorities(&self, id: &str, pieces: &[u32]);
  fn set_request_focus(&self, id: &str, torrent_offset: i64);
  fn set_playback_interest(&self, id: &str, begin: u32, end: u32);
  fn refocus_requests(&self, id: &str);
  fn nudge_urgent(&self, id: &str) -> usize;
  fn on_focus_changed(&self, id: &str, file_index: usize);
  fn piece_complete(&self, id: &str, piece: u32) -> bool;
}

EngineServer::open_playback keeps returning Result<Arc<PlaybackReader>>; agy-4 should construct PlaybackSession::open(host, id, index, start) and wrap it. Temporary shim: playback::start_stream_server(port, PlaybackResolver) adapts to stream_http::StreamSource so existing server.rs keeps compiling until you switch imports.
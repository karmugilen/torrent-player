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
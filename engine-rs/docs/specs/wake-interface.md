Coordinator: agreed wake interface for the connection driver (codex-4 implements the driver side in src/conn, codex-1 the handler side in peer_conn.rs/torrent.rs). Build on it as written; reply only if it cannot work.

In src/conn (codex-4):
  trait ConnHandler {
      ... existing methods ...
      /// Called when the torrent asked for a request update (ConnHandle::wake) or when wake_interval elapses.
      fn on_wake(&mut self, handle: &ConnHandle) {}
      /// Periodic wake while Some; re-read after every callback. Used for Webtor HOL rescue / hedging / steal ages during playback.
      fn wake_interval(&self) -> Option<std::time::Duration> { None }
  }
  impl ConnHandle {
      /// Coalescing, non-blocking: many calls before the driver runs produce one on_wake.
      pub fn wake(&self);
  }
  The driver selects on: incoming message, wake notify (tokio::sync::Notify), and a sleep of wake_interval() when Some. Never call on_wake while the handler is already running; never busy-loop.

In wt-core torrent side (codex-1):
  - PeerConnHandler implements on_wake (recompute desired requests, interest, Webtor HOL rescue / hedge / steal / yield) and wake_interval (Some(100 ms) while the torrent has Now/Next pieces or outstanding urgent requests, else None).
  - TorrentInner calls handle.wake() on every connection when: piece or file priorities change, reader positions change (seek), set_request_focus / refocus_requests / nudge_urgent_requests, pause or resume (allow/disallow download), a piece hash result arrives, a connection closes (its requests become free), and when the request order changes.

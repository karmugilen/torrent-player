//! Host-facing engine API consumed by the JNI exports.
//!
//! Mirrors the behaviour of `engine-go/main.go` (android/jnitest build) so that
//! `EngineHost.kt` can load `libengine.so` unchanged. agy-4 implements this
//! trait over `EngineServer`; the JNI layer holds a single global instance.

/// Engine operations exposed to Android via JNI.
///
/// Every method documents the Go semantics from `engine-go/main.go`. Return
/// values and error codes must stay bit-compatible with the Kotlin decoder in
/// `android/app/src/main/java/webtor/app/EngineHost.kt`.
pub trait EngineHostApi: Send + Sync + 'static {
    /// Start the native engine.
    ///
    /// Go: `Java_webtor_app_EngineHost_startEngine` → `initializeAndroidEngine`.
    ///
    /// - `download_dir` is the UTF-8 absolute path bytes from Kotlin
    ///   (`downloadDir().absolutePath.toByteArray(UTF_8)`).
    /// - Empty download dir → log error, return `1`.
    /// - `max_peers <= 0` → treat as `55`.
    /// - Already started → return `0` (idempotent).
    /// - Creates `EngineServer(ctl_port=0, download_dir, max_peers)`, starts
    ///   streaming (`StartStreaming`), stores the global instance, notifies
    ///   change listeners, and returns `0` on success or `1` on failure.
    fn start_engine(&self, download_dir: &[u8], max_peers: i32) -> i32;

    /// Dispatch a control HTTP-style request over JNI.
    ///
    /// Go: `Java_webtor_app_EngineHost_requestEngine`.
    ///
    /// Returns a byte payload framed as `"{status}\\n{body}"` where `status` is
    /// a decimal HTTP status and `body` is the raw response bytes (usually
    /// JSON). Kotlin splits on the first `\\n` (`EngineHost.request`).
    ///
    /// When the engine is not yet started, Go returns
    /// `b"503\\n{\"error\":\"engine is starting\"}"`.
    fn request_engine(&self, method: &str, path: &str, body: &[u8]) -> Vec<u8>;

    /// Block until the engine change version advances past `after_version`.
    ///
    /// Go: `Java_webtor_app_EngineHost_waitForEngineEvent`.
    ///
    /// - Engine not started → sleep 250 ms and return `after_version`.
    /// - `timeout_millis <= 0` or `> 60_000` → clamp to 30 seconds.
    /// - Otherwise wait up to that duration and return the current version
    ///   (`EngineServer.WaitForChange`).
    fn wait_for_engine_event(&self, after_version: i64, timeout_millis: i64) -> i64;

    /// Open a playback reader for torrent `id` at `file_index`.
    ///
    /// Go: `Java_webtor_app_EngineHost_openPlayback`.
    ///
    /// Returns a positive opaque handle, or `0` when the engine is missing or
    /// `OpenPlayback` fails (Go logs the error and returns 0).
    fn open_playback(&self, id: &str, file_index: i32) -> i64;

    /// Read up to `buf.len()` bytes at `offset` from a playback handle.
    ///
    /// Go: `Java_webtor_app_EngineHost_readPlayback`.
    ///
    /// - Unknown handle, `size < 0`, `size > 1 MiB`, or `size` larger than the
    ///   Java `target` array → return `-1`.
    /// - Read error → log and return `-1`.
    /// - Success → copy `n` bytes into the Java array (`SetByteArrayRegion`)
    ///   and return `n` (`0` is a valid short/EOF-style read).
    ///
    /// The JNI layer passes a mutable slice whose length is the requested
    /// `size`; implementors write into that slice and return the byte count.
    fn read_playback(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32;

    /// Close a playback handle.
    ///
    /// Go: `Java_webtor_app_EngineHost_closePlayback`.
    /// Missing handles are ignored; a present handle is deleted and `Close`d.
    fn close_playback(&self, handle: i64);

    /// Open a preview reader for torrent `id` at `file_index`.
    ///
    /// Go: `Java_webtor_app_EngineHost_openPreview`.
    /// Same handle/`0` semantics as [`open_playback`].
    fn open_preview(&self, id: &str, file_index: i32) -> i64;

    /// Read preview bytes into `buf`.
    ///
    /// Go: `Java_webtor_app_EngineHost_readPreview`.
    /// Same validation and `-1`/`n` return codes as [`read_playback`]. Go also
    /// rejects a negative `n` from the reader (returns `-1`).
    fn read_preview(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32;

    /// Close a preview handle.
    ///
    /// Go: `Java_webtor_app_EngineHost_closePreview`.
    /// Missing handles are ignored; a present handle is deleted and `Close`d.
    fn close_preview(&self, handle: i64);
}

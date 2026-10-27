# wt-engine PORT_NOTES (JNI / host / Android build)

Owned by the JNI helper worker (`src/jni/`, `src/bin/main_host.rs`,
`engine-rs/build-android.sh`, `tests/jni_*.rs`). agy-4 owns the rest of the crate.

## Ported

- **`src/jni/host_api.rs`**: `EngineHostApi` trait mirroring `engine-go/main.go`
  JNI-facing behaviour (start / request / wait / open-read-close playback &
  preview). Doc-comments capture Go return codes and `EngineHost.kt` framing
  (`"{status}\n{body}"`).
- **`src/jni/mod.rs`**: nine `#[no_mangle] pub extern "system" fn
  Java_webtor_app_EngineHost_*` exports, `OnceLock` global engine set via
  `startEngine` (factory or default `AndroidEngineHost`), `catch_unwind` on
  every export, `SetByteArrayRegion` for read paths (single Rust buffer),
  Android logging via `log` + `android_logger` under `cfg(target_os =
  "android")`.
- **`src/jni/server_impl.rs`**: `AndroidEngineHost` implements `EngineHostApi`
  over `EngineServer` (tokio runtime + `start_streaming`, control framing,
  handle tables). Opens playback/preview through public server fields so the
  JNI layer stays compiling while agy-4 settles `open_*` return types.
- **`src/ctl_http.rs`**: localhost control HTTP (`127.0.0.1:<port>`) forwarding
  to `EngineServer::request`; `POST /shutdown` closes and signals Done.
- **`src/bin/main_host.rs`**: `engine-host` binary with `-port`/`--port`
  (default 18080), `-path`/`--path`, `-max-peers`/`--max-peers` (55).
  Wires `EngineServer::new` → `start_streaming` → `ctl_http::serve` → wait
  for SIGINT/SIGTERM or Done → `close`. Verified with `curl /stats`.
- **`engine-rs/build-android.sh`**: NDK 26.1.10909125, `ENGINE_ABI` arm64-v8a /
  x86_64, cargo release cdylib with NDK clang linker + ring `CC_*`/`AR_*`,
  `RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384`, stamps
  `WEBTOR_APP_VERSION`/`APP_VERSION` from `versionName`, `llvm-strip
  --strip-unneeded`, copies to `android/app/src/main/jniLibs/$ABI/libengine.so`.
- **`tests/jni_exports.rs`**: fake `EngineHostApi` semantics; optional JVM
  invocation smoke test (skips if no JVM); `nm`/`llvm-nm -D` symbol check for
  the nine exports (skips if tool or `.so` missing).

## Differs from Go

- Host API is a Rust trait + `AndroidEngineHost` / factory so JNI stays
  decoupled from unfinished server wiring. Go inlines everything in `main.go`.
- `main_host` accepts both `-flag` and `--flag`; Go's `flag` package is
  single-dash only.
- Android logging uses the `log`/`android_logger` crates (tag
  `webtor-engine`).

## Helper modules (stream_http / preview / live_preview / network / playback)

Owned by the stream/preview/playback helper worker (this turn: `playback.rs`
taken over from agy-4; old inline stream server removed).

### Ported

- **`src/stream_http.rs`**: hyper 1 stream server; Go URL
  `/torrent/{id}/file/{index}`; single-range `206` / `Content-Range` /
  `Accept-Ranges` / `416`; `HEAD`; CORS + cache headers external players
  need; MIME via extension map + `wt_policy::is_video_file_name`;
  `StreamSource` / `StreamReader` trait seam (fake `BytesStreamReader` for
  tests; agy-4 wires EngineServer playback readers). `range_start` matches
  Go. Chunked body with client-disconnect cancel via channel drop.
- **`src/preview.rs`**: `PreviewReader` with 8 MiB budget, idempotent
  close, `preview_dir`, `admit_preview_range` (skip complete pieces,
  payload gate, budget cap). Session timeout constant `10s`.
- **`src/live_preview.rs`**: `build_preview_state` /
  `preview_cache_signature` / `pieces_overlapping_file` for
  `/preview-state` JSON (`active`, `byteOffset`, hex `signature`).
- **`src/network.rs`**: `NetworkManager` generation gate (ignore
  stale/duplicate), `parse_change_body` (requires `connected`),
  `NetworkDialer` epoch/reset/track (+ optional TCP dial), recovery
  delay constants from Go.
- **`src/playback.rs`**: Go `playback.go` orchestration on real policy +
  backend seam. `PlaybackHost` trait (agy-4 implements on `EngineServer`):
  torrent snapshot, settings, `open_file_reader`, piece priorities,
  `set_request_focus` / `refocus_requests` / `nudge_urgent` /
  `set_playback_interest`, focus-change hook. `ReaderBackend` +
  `FakeReaderBackend` (unit tests) and `playback_core::CoreReaderBackend`
  (`wt_core::reader::Reader` with `set_readahead` / `set_readahead_fn` /
  `set_webtor_responsive`). `PlaybackPriorityCoordinator` (max-demand,
  multi-reader High demotion, active-per-file park). `PlaybackSession`
  open/update_window/read/close with Now/Next/Readahead/High, video
  head/tail (`wt_policy` spans), AdaptiveBuffer, ResumeRange, NextPreload,
  IdleDirectPlay, SeekLatencyStats, scrub debounce. `PlaybackReader`
  implements `StreamReader`; `open_for_host` returns frozen
  `(handle, Arc<_>)` for EngineServer. Temporary `start_stream_server`
  shim + `PlaybackResolver` keep agy-4 compiling while it switches to
  `stream_http` + `PlaybackHost`.
- **`src/playback_core.rs`**: `CoreReaderBackend` wrapping
  `File::new_reader()`.

### Differs from Go

- Stream body is pulled through a trait instead of opening
  `playbackHTTPReader` inside the handler; agy-4 supplies the source.
- Preview demand publishing / temp storage attach stay in EngineServer
  once storage/core land; this module owns the reader + admission math.
- Network recovery announce/DHT loops remain EngineServer-owned; this
  module owns the dialer + generation gate the server calls.
- Playback piece I/O goes through `ReaderBackend` so unit tests run
  without peers; Go inlines anacrolix readers. Resume-range disk
  persistence and full `/pieces` grid HTTP remain host/server-owned.
- `engine_playback*` integration suites stay `#[ignore]` until agy-4
  implements `PlaybackHost` and `open_playback` delegates to
  `PlaybackReader::open_for_host` / `PlaybackSession::open`.

### PlaybackHost signature (for agy-4)

```rust
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
```

Typical `open_file_reader`: `Ok(Box::new(CoreReaderBackend::from_file(&file)))`.
Typical `open_playback` body: allocate handle →
`PlaybackReader::open_for_host(host, coord, id, index, start, handle)` →
insert Arc → return `(handle, arc)`.

## Core Engine & Control API (server / trackers / types / seeding / connections)

Owned by agy-4 (playback.rs ownership moved to the helper worker above).

### Ported

- **`src/server.rs`**: `EngineServer` core control route dispatcher (`dispatch_control`), condvar change event notifications (`wait_for_change`), public constructor `new(ctl_port, download_dir, max_peers)` and public request entry point `request(method, path, body) -> (u16, Vec<u8>)`. Implements `EngineHostApi` for JNI integration. Handles all control routes: `stats`, `add`, `torrent/{id}`, `play`, `file-only`, `focus`, `configure`, `select`, `pause`, `resume`, `remove`, `forget-peers`, `pieces/{id}`, `preview-state/{id}`, `connections/{id}`, `metadata/{id}`, `settings`, `network`, `bypass`, `cache`, and `shutdown`. Still uses the playback shim (`PlaybackReader::new` stub + `start_stream_server` resolver) until `PlaybackHost` is wired.
- **`src/types.rs`**: Serde models byte-compatible with Go and Kotlin `EngineClient`: `StatsResponse`, `TorrentView`, `FileView`, `PlayRequest`, `ConfigureRequest`, `SettingsResponse`, `SettingsRequest`, `AddRequest`, `AddResponse`, `PieceTelemetryResponse`, etc.
- **`src/trackers.rs`**: Default public announce list, tracker URL normalization/validation, passkey preservation, tier cleaning (`clean_supplied_trackers`), and `TrackerPool` persistence.
- **`src/tracker_status.rs`**: Live announce status tracking (`waiting`, `working`, `error`), peer count formatting, and view generation.
- **`src/connections.rs`**: Live peer connections view and GeoIP country resolution via pure-Rust `maxminddb`.
- **`src/seeding.rs`**: `.seed-accounts.json` persistence and seeding ratio/time limits.
- **`src/version.rs`**: Version metadata reader returning build version or `"dev"`.

### Verification Status (playback helper slice)

- `cargo test -p wt-engine --lib` → 29 passed (6 playback orchestration + stream/preview/network/server/trackers units).
- `cargo test -p wt-engine --test engine_playback_grid` → `test_range_start_parses_single_ranges` pass; other two ignored pending host/HTTP.
- Remaining `engine_playback*`, `engine_direct_playback`, `engine_instant_seek`, `engine_idle_buffer`, `engine_playback_resume`, `engine_next_preload`, `engine_seek_http` stay ignored until server implements `PlaybackHost`.
- `cargo clippy -p wt-engine --lib --no-deps -- -D warnings` clean on owned modules.
- `cargo fmt` applied on owned files.

## Two-Host Engine Transfer & Parity (agy-engine)

- **`tests/engine_two_host.rs`**: Full end-to-end multi-process engine transfer on localhost:
  - Spawns two independent `engine-host` instances (seeder and leecher) on ephemeral ports.
  - Seeds a pre-verified download via `.boundary` and checkpoint files.
  - Adds torrent to leecher via magnet link with `x.pe=<seeder_ip>:<seeder_port>`.
  - Leecher downloads metadata, configures storage, downloads all pieces, reaches 100% verified bytes.
  - Verified disk bytes on leecher match fixture data byte-for-byte.
- **`cargo run -p wt-parity -- api-diff`**:
  - **30 MATCH, 0 DIFF, 0 CRITICAL**.
  - All routes (`/stats`, `/settings`, `/network`, `/add`, `/configure`, `/select`, `/pause`, `/resume`, `/remove`, `/torrent/{id}`, `/metadata/{id}`, `/connections/{id}`, `/pieces/{id}`, etc.) byte/JSON-parity matched against `engine-go`.
- **Android `libengine.so`**:
  - `bash engine-rs/build-android.sh` produces `libengine.so` (arm64-v8a).
  - 16 KiB ELF segment aligned (`0x4000`).
  - Contains all 9 JNI exports (`Java_webtor_app_EngineHost_*`).
- **Core / Server Integration Fixes**:
  - `apply_transfer_state`: updates file and piece priorities according to `payload_transfers_allowed`, `file_only`, and `selected` files.
  - `/settings`: `maxPeers: 0` treated as unchanged; `claudeSeekMode` defaults to false and resets `seek_stats` on toggle with `set_urgent_request_stealing`.
  - `/network`: rejects requests without `connected`.
  - `/connections`: queries active connections from `rec.torrent.peer_conns()`, normalizes localhost address, resolves GeoIP country, and sorts.
  - Payload gate: returns 409 `"payload transfers are disabled in settings"` on incomplete playback when transfers disallowed.
  - Tracker dialer: tracks raw file descriptors in `NetworkDialer` and issues `libc::shutdown(fd, SHUT_RDWR)` on reset.

## Integration Test Suite (agy-tests)

Owned by `agy-tests` (`tests/engine_*.rs`, `tests/support/`, `tests/ENGINE_TESTS.md`, `tests/types_json.rs`, except `engine_two_host.rs` and `jni_exports.rs`).

### Status & Parity Inventory

- Total integration tests mapped from `engine-go`: **143 tests** (across 27 test files).
- **Pass**: **51 tests** running and passing cleanly with parity against Go.
- **Waiting**: **92 tests** faithfully ported with full Go setup, assertions, and timeouts, currently marked `#[ignore = "needs: <feature>"]` pending engine feature delivery from `agy-engine` and `agy-core`.
- **Placeholders (`unimplemented!()` or empty panics)**: **0** (completely eliminated; `grep -c unimplemented` = 0).
- **Fails**: **0**.
- Test suite compilation: `cargo test -p wt-engine --tests --no-run` compiles all test targets cleanly.
- Code quality: `cargo clippy --tests -p wt-engine -- -D warnings` and `cargo fmt` clean.
- Cross-worker requests: all waiting tests are cataloged in `engine-rs/docs/requests.md` addressed to `agy-engine` or `agy-core`.

## Round 2 Feature Delivery (agy-engine)

- **`status_fingerprint`**: Implemented `status_fingerprint(&self) -> [u8; 32]` on `EngineServer` calculating SHA-256 fingerprint over speeds and per-torrent status (id, paused, checking, configured, selected, peers, piece completion, stats, direct play idle status, next preload).
- **Preview Demand Release**: Wired target-aware `close_preview_for(&self, id: &str)` into `POST /pause`, `POST /configure`, and `POST /remove`. Pausing an unrelated torrent preserves active preview; pausing the active preview's torrent closes the reader and releases temporary piece demand.
- **`TorrentRecord::refresh` and `EngineServer::self_ref`**: Made `pub(crate)` to satisfy cross-module preview demands without violating encapsulation.
- **Parity & Tests**: 34 needs-ignored tests verified passing with `--ignored`; `cargo test -p wt-engine` passing 51/51; `engine_two_host` passing; `wt-parity api-diff` verified 30 MATCH / 0 DIFF / 0 CRITICAL.



## Focused server policy audit (2026-10-27, codex-engine)

The revised task covers startup safety and Go-equivalent server limits; real
swarm peer/throughput work belongs to grok-wire and grok-swarm.

- Connection defaults match Go: maxPeers 55 on Android (constructor 0 means
  100, cap 500), dial rate/burst 30, half-open 24 per torrent / 48 total,
  dial timeout 8 s nominal / 2 s minimum, handshake timeout 5 s.
- Validate RAM and peer settings before changing live peers; preserve a resting
  torrent's zero connection cap. Unrelated settings no longer reset file/piece
  demand or upload admission. Checking/closed torrents cannot enable payload.
- Negative transfer rates retain their previous value; positive bursts follow
  Go's 1-byte to 1-MiB bounds, and zero means unlimited without resetting burst.
  Signed dial requests clamp to 10–80; idle and seeding caps follow Go bounds.
- The policy sampler now publishes existing per-torrent transfer-rate samples.
  Incomplete downloads retain reciprocal uploads with seeding disabled.
- Added eight policy/input regressions and a real AndroidEngineHost warm-start
  regression: cached contacts, no caller Tokio runtime, three starts and
  idempotent start calls, plus /stats, /settings, /version and /cache.
- Coordinator approved ownership of engine_swarm_playback.rs: seed fixtures
  explicitly enable seeding before configure and keep readable/writable files;
  leecher assertions and timeouts are unchanged.

Validation: 46 library tests pass; engine_magnet_configure (2), engine_server
(9, 2 still ignored), engine_settings (4), and engine_two_host (1) pass.
The two-host magnet reaches 262144 verified bytes with done=true.
API diff remains 30 MATCH / 0 DIFF / 0 CRITICAL. Engine all-target Clippy with
--no-deps and -D warnings passes. Full dependency Clippy is blocked by the
active wt-core conn/dialer.rs doc_lazy_continuation warning, reported to its owner.

Full cargo test -p wt-engine still fails the two engine_swarm_playback tests:
after the fixture correction, the first read stalls after 4 MiB of verified
startup data and can lose all live peers; Claude seek stalls at a later offset.
The core owner has the reproducible command and failure log via requests.md.
No public peers_compare runs were made, as requested. Older needs-feature
backlogs remain deferred under the coordinator's focused task.

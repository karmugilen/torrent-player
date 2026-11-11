# HANDOFF: waist / perf 2.4.2

Phone-sideload build candidate (`versionName` 2.4.2 / `versionCode` 102). Cuts Kotlin↔Rust control-plane waste on add, library keys, and the piece heat-grid refresh. Worktree left dirty; do not push.

## Files changed

### Rust (`engine-rs/crates/wt-engine`)
- `src/pieces_bin.rs` — encode/decode for `WTPC` v1 little-endian pieces snapshot
- `src/telemetry.rs` — shared `build_piece_telemetry`; JSON `/pieces` + binary `/pieces-bin`
- `src/server.rs` — `dispatch_control_bytes`; routes `POST /add-bytes`, `GET|POST /normalize`, `GET /pieces-bin/{id}`; `add_from_torrent_bytes` / `register_torrent_spec`
- `src/ctl_http.rs` — pass raw body bytes; `application/octet-stream` for successful pieces-bin
- `src/jni/server_impl.rs` — no longer UTF-8-lossy the request body (required for raw torrents)
- `src/jni/host_api.rs` — docs for binary body/response via existing `requestEngine`
- `src/lib.rs` — `pub mod pieces_bin`
- `src/types.rs` — `PartialEq` on piece telemetry types
- `tests/engine_server.rs` — add-bytes + pieces-bin round trip; normalize magnet/base32/hex
- `tests/support/engine.rs` — `dispatch_bytes`

### Android
- `android/core/.../EngineClient.kt` — `requestRaw`, `addBytes`, `normalize`, binary `pieces` prefer `/pieces-bin` with JSON fallback, `parsePiecesBin`
- `android/core/.../EngineClientTest.kt` — binary pieces, addBytes, normalize tests
- `android/app/.../EngineHost.kt` — binary-safe `requestRaw` framing (`status\n` + raw body)
- `android/app/.../LibrarySession.kt` — file/link adds use raw bytes; library hash via `normalize` (local fallback)
- `android/app/.../TorrentFetch.kt` — `decodeTorrentData`; Base64 encode kept for stored `/metadata`
- `android/app/.../DownloadRestore.kt` — restore decodes Base64 then `addBytes`
- `android/app/.../Formatters.kt` — `normalizeTorrentIdLocal`

### Release metadata
- `android/app/build.gradle.kts` — 2.4.2 / 102
- `fastlane/metadata/android/en-US/changelogs/102.txt`
- `glossary.md` — brief waist note

## Binary pieces format (`WTPC` v1)

Little-endian. Magic `WTPC` (4) + version `u16=1` + flags `u16` (bit0 = ghostPro) + generation `i64` + timestamp `i64` + totalPieces `u32` + maxBuckets `u32` + pieceLength `i64` + lastPieceLength `i64` + idLen/infoHashLen/bucketCount/playbackCount/playheadCount/detailStatesLen (`u16` each) + detailStart/detailPlayhead/reserved (`u32` each). Then id UTF-8, infoHash UTF-8 hex (may be empty), buckets (`8×u32` each: start,end,total,selected,verified,receiving,selectedVerified,selectedReceiving), playback (`start u32`, `end u32`, priority `u8` U/N/R/L/B + 3 pad), playheads (`u32` each), detail states UTF-8.

JSON `GET /pieces/{id}` remains.

## How to verify

```bash
# Rust (use shared target dir; keep jobs low)
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-waist CARGO_BUILD_JOBS=2 \
  cargo test -p wt-engine --manifest-path engine-rs/Cargo.toml --lib pieces_bin -- --test-threads=1
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-waist CARGO_BUILD_JOBS=2 \
  cargo test -p wt-engine --manifest-path engine-rs/Cargo.toml --test engine_server add_bytes_raw -- --test-threads=1
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-waist CARGO_BUILD_JOBS=2 \
  cargo test -p wt-engine --manifest-path engine-rs/Cargo.toml --test engine_server normalize_magnet -- --test-threads=1

# Kotlin (skip native rebuild for unit tests)
JAVA_HOME=/usr/lib/jvm/java-21-openjdk ANDROID_HOME=/home/kar/Android/Sdk \
  ./android/gradlew -p android :core:test :app:testDebugUnitTest \
  --tests 'webtor.core.EngineClientTest' --tests 'webtor.app.TorrentFetchTest' \
  --tests 'webtor.app.DownloadRestoreTest' -x buildRustEngine --no-daemon
```

Coordinator builds/signs the phone APK (needs `buildRustEngine` so JNI picks up raw-body/`pieces-bin`).

## Risks

- Heat-grid path now tries `/pieces-bin` first every ~1 Hz; on older engines without the route, each tick pays one 404 then JSON (acceptable; new APK ships both).
- `POST /add-bytes` requires a binary-safe JNI body path; host HTTP also passes raw bytes. JSON `/add` + base64 still works for restore/compat.
- Normalize is best-effort from the app before/while the engine is ready; local Base32→hex fallback remains for duplicate detection.
- Playback overlay fields are included in `WTPC` v1; if a future field is needed, bump version and keep Kotlin fallback to JSON.

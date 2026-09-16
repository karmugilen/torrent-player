# Handoff: Go BitTorrent Engine Migration (`webtor-go`)

## Resume recovery update — 2026-09-16

Version 1.4.5 (22) fixes the saved-data verification timeout on large downloads.
Verification now runs outside the JNI request deadline, reports progress and
honors pause/resume/removal. See `docs/RESUME_RECOVERY_PLAN.md`,
`docs/RELEASE_1.4.5.md` and `artifacts/RELEASE_1.4.5_TEST_REPORT.md`.
The 1.4.4 F-Droid submission passed all nine jobs; maintainer acceptance remains
pending.

## Tracker release update — 2026-09-15

Version 1.4.4 (21) adds background public tracker fetching and persistent caching.
See `docs/TRACKER_DISCOVERY_PLAN.md`, `docs/RELEASE_1.4.4.md`, and
`artifacts/RELEASE_1.4.4_TEST_REPORT.md` for the implementation, limits and evidence.
GitHub authentication has been restored. The existing F-Droid MR is !48893; its
old 1.4.3 Node build passed, with a formatting-only pipeline failure.

## Previous build update — 2026-09-15

This section supersedes the historical migration notes below.

- Android control is now direct JNI through `EngineHost` and `EngineClient`; no fixed port 18080 or OkHttp dependency remains in the app. The standalone Go host retains its development HTTP adapter.
- Native change signals replace Kotlin status/metadata/piece polling. One process-owned observer delivers cancellable StateFlow notifications; Go's existing one-second rate sampler detects telemetry changes, including final hash completion and idle peer changes.
- Playback uses `PlaybackProvider` with exact name, size, MIME and read grants. Completed files open the saved descriptor; unfinished files use an Android proxy descriptor reading verified bytes through JNI with persistent seek/readahead state. The HTTP media endpoint remains available.
- File completion now excludes unverified chunks. The content handoff refreshes MediaStore metadata where possible and detects a saved file shorter than expected. This does not repair a file that was already corrupted by an older build.
- Removed both Adwaita font assets and unused anet `mobile.aar`/sources JAR. The app uses the Android system font. Direct UUID use is removed, but `google/uuid` remains required by Pion ICE/WebRTC.
- Go regression tests cover shared boundaries, waiting for verified data, close cancellation, range requests, final completion, events and local peer transfer. A generated H.264/AAC MP4 passes through a local TCP seed, the destination storage and playback reader, then decodes with FFmpeg.
- `JAVA_HOME=/usr/lib/jvm/java-21-openjdk bash scripts/test-jni.sh` exercises the actual JNI exports on the host JVM, including descriptor duplication and Unicode paths. Android framework proxy behavior and the user's media player still require device testing.
- Final distributable: `artifacts/torrent-player-optimized-playback.apk` (release optimization, locally signed with the same debug key used for the previous phone install). Build from `android/` with `./gradlew :core:test :app:testDebugUnitTest :app:assembleRelease -Pwebtor.localSigning=true`.

No public-swarm throughput or battery improvement is claimed from these local checks. Peer protocols, trackers and connection limits are preserved by this optimization pass.

## Historical migration notes

## 1. Executive Summary

This project branch eliminates the embedded **Node.js 18 & Google V8** runtime (`nodejs-mobile`, `libnode.so`, native C++ addons `utp_native` and `node_datachannel`), replacing it with a pure, high-performance native **Go BitTorrent engine** based on [`anacrolix/torrent`](https://github.com/anacrolix/torrent).

### Key Outcomes
- **Build Time**: Reduced from **~25 minutes** (V8 compilation) to **~23 seconds**.
- **Binary Footprint**: Removed ~50MB of uncompressed `libnode.so` and thousands of JavaScript/node_modules asset files.
- **First Launch**: Zero asset decompression on first launch (no extracting files to `filesDir/nodejs-project`).
- **Swarm Performance**: Standard BitTorrent TCP + uTP + WebRTC + DHT + PEX with multi-threaded disk I/O and zero V8 garbage collection jitter.
- **API Parity**: 100% compatible with the existing Kotlin frontend via local HTTP JSON REST API (`127.0.0.1:18080`).

---

## 2. Worktree & Git Reference

- **Location**: `/run/media/kar/Turbo/webtor-go`
- **Branch**: `go-engine`
- **Base Commit**: `8d8acbf` (master branch at `/run/media/kar/Turbo/webtor` remains clean and untouched)

---

## 3. Ready-to-Test APK

- **File Path**: [`/run/media/kar/Turbo/webtor-go/android/app/build/outputs/apk/debug/app-debug.apk`](file:///run/media/kar/Turbo/webtor-go/android/app/build/outputs/apk/debug/app-debug.apk)
- **Size**: ~28 MB (Debug build)
- **Target ABI**: `arm64-v8a` (Android minSdk 26, targetSdk 34)
- **Native Library**: `lib/arm64-v8a/libengine.so` (100% self-contained, depends only on Android standard bionic libraries: `libc.so`, `libm.so`, `libdl.so`, `liblog.so`)

### Quick Install Command
```bash
adb install -r /run/media/kar/Turbo/webtor-go/android/app/build/outputs/apk/debug/app-debug.apk
```

---

## 4. Stability & Performance Bug Fixes (Latest Update)

### Fix A: "Unexpected End of Stream" During Playback
**Root Causes:**
1. **`reader.SetResponsive()` Hazard**: Responsive reading mode in `anacrolix/torrent` returns unverified/partial chunk data prematurely before SHA-1 hash piece verification finishes. When ExoPlayer's container parser encountered truncated or corrupt blocks, it aborted playback with `"unexpected end of stream"`.
2. **Missing Readahead Binding**: `anacrolix/torrent` defaults `readaheadFunc` to an internal dynamic algorithm that throttles seek buffering. Calling `reader.SetReadahead()` alone without clearing the function caused readahead to be ignored.
3. **Premature Filesystem EOF on Seeking**: Target files on Android storage were not truncated to their full declared length upon descriptor attachment or fallback creation. When ExoPlayer seeked to read the file tail (such as MP4 `moov` atom), `f.ReadAt` immediately hit filesystem `io.EOF`.

**Solutions Applied:**
- **Removed `reader.SetResponsive()`**: Reader now guarantees strictly verified, piece-validated blocks.
- **Configured Fixed Readahead**: Added `reader.SetReadaheadFunc(nil)` and `reader.SetReadahead(20 * 1024 * 1024)` so 20MB is continuously prefetched ahead of the playhead.
- **Request Context Binding**: Added `reader.SetContext(r.Context())` so aborted client requests cleanly release resources.
- **Full File Truncation**: Added `f.Truncate(f.Length)` on file descriptor attachment and fallback storage creation so seeks to any offset within the file boundary never hit filesystem EOF prematurely.
- **Zero-Padded Partial Reads**: Updated `readFromFiles` so that if an unwritten block is read, it returns clean zero bytes rather than unhandled partial EOFs.
- **Automatic Fallback Activation**: Added automatic fallback filesystem activation in `handlePlay` and `handleStream` if the storage was not previously configured.

---

### Fix B: Erratic / Inconsistent Download Speeds (100 KB/s jumping to 4 MB/s)
**Root Causes:**
1. **Network Stack & Router Bufferbloat**: Setting `TotalHalfOpenConns = 500`, `HalfOpenConnsPerTorrent = 200`, and `DialRateLimiter = rate.Inf, 1000` combined with a 2-second DHT burst loop flooded the Android network interface and home Wi-Fi router NAT state tables. Packet drops caused active TCP transfers to enter fast-retransmit and congestion window collapse down to 100 KB/s, then spike to 4 MB/s once the dial queue drained.
2. **Piece-Level Metric Quantization**: `rateTrackerLoop` was calculating speeds using `BytesReadUsefulData`. In BitTorrent, `UsefulData` only increments when an entire multi-megabyte piece completes SHA-1 hash verification. While downloading a 2MB piece, the speed appeared as 0 or 100 KB/s, and the moment the piece completed verification, the speed spiked to 4 MB/s.

**Solutions Applied:**
- **Balanced Concurrency Limits**:
  - `DialRateLimiter`: Set to `rate.NewLimiter(30, 30)` (30 dials/second steady rate).
  - `HalfOpenConnsPerTorrent`: Reduced from 200 to 40.
  - `TotalHalfOpenConns`: Reduced from 500 to 80.
  - `TorrentPeersHighWater`: Reduced from 5000 to 1000; `TorrentPeersLowWater`: 200.
- **Paced DHT Announcements**: Replaced the 2-second burst loop with a single clean initial announce and a 30-second periodic ticker, eliminating UDP socket storms.
- **Chunk-Level Real-Time Speed Tracking**: Changed metric calculation to `stats.ConnStats.BytesReadData.Int64()`, capturing byte arrival in real time as chunks land over the wire.
- **Exponential Moving Average (EMA)**: Applied a 60/40 EMA filter (`0.6 * prev + 0.4 * current`) to eliminate display flutter and report smooth, stable throughput.

---

## 5. Architecture & Implementation Summary

### A. The Go Engine (`engine-go/`)
Located at [`/run/media/kar/Turbo/webtor-go/engine-go`](file:///run/media/kar/Turbo/webtor-go/engine-go):

1. **[`server.go`](file:///run/media/kar/Turbo/webtor-go/engine-go/server.go)**:
   - **Control Server (`127.0.0.1:<ctlPort>`)**:
     - `GET /stats`: Download/upload speeds, active torrents, progress.
     - `POST /add`: Accepts magnet links, `.torrent` paths, or base64 data; supports `prepare: true`.
     - `GET /torrent/:id`: Returns metadata, file lists, peer counts, progress.
     - `POST /play`: Prioritizes requested file pieces (head + tail) and returns streaming URL.
     - `POST /configure`: Attaches SAF file descriptors and activates download priority.
     - `POST /select`: Dynamically adjusts piece priorities for selected files.
     - `POST /pause` / `POST /resume`: Controls download activity.
     - `POST /remove`: Drops torrent and cleans up storage.
     - `GET /pieces/:id?maxBuckets=256`: Computes piece buckets (verified, receiving, selected) for UI telemetry.
     - `GET /metadata/:id`: Returns base64 encoded `.torrent` file.
     - `GET /settings` / `POST /settings`: Queries and updates `maxPeers`.
     - `POST /shutdown`: Graceful shutdown.
   - **Streaming Server (`127.0.0.1:<streamPort>`)**:
     - `/torrent/:id/file/:index`: Serves media files with `Range` header support (`http.ServeContent`) and 20MB readahead for smooth seeking in ExoPlayer and VLC.

2. **[`storage.go`](file:///run/media/kar/Turbo/webtor-go/engine-go/storage.go)**:
   - **SAF File Descriptor Bridge**: Implements `storage.ClientImpl` for `anacrolix/torrent`.
   - **In-Memory Prefetch Cache**: When `prepare: true`, caches initial pieces up to 48MB in memory while metadata and initial pieces arrive.
   - **Descriptor Attachment**: When Android passes open file descriptors via `POST /configure`, duplicates the descriptors (`syscall.Dup`), pre-allocates file lengths (`Truncate`), and flushes cached pieces to storage.
   - **Automatic Fallback Storage**: If no SAF descriptors are provided (e.g. streaming playback mode), falls back seamlessly to app-private cache storage.

3. **[`types.go`](file:///run/media/kar/Turbo/webtor-go/engine-go/types.go)**:
   - Strict JSON schemas matching `webtor.core.EngineClient`.

4. **[`main.go`](file:///run/media/kar/Turbo/webtor-go/engine-go/main.go)**:
   - **JNI Export**: `Java_webtor_app_EngineHost_startEngine` compiled directly into the shared library symbol table.

5. **[`build.sh`](file:///run/media/kar/Turbo/webtor-go/engine-go/build.sh)**:
   - Cross-compiles for Android `arm64-v8a` using NDK Clang r26b with `-static-libstdc++`.
   - Output: `android/app/src/main/jniLibs/arm64-v8a/libengine.so`.

---

## 6. Verification & Test Results

1. **Native Shared Library**:
   `libengine.so` compiled cleanly (23 MB). Dynamic dependencies verified:
   ```
   Shared library: [liblog.so]
   Shared library: [libdl.so]
   Shared library: [libm.so]
   Shared library: [libc.so]
   ```
   Zero external C++ runtime dependencies.
2. **Kotlin Unit Tests**:
   Ran `./gradlew :core:test`: All 13 tests in `:core` PASSED.
3. **APK Assembly**:
   Ran `./gradlew :app:assembleDebug`: Built in 23s at `/run/media/kar/Turbo/webtor-go/android/app/build/outputs/apk/debug/app-debug.apk`.

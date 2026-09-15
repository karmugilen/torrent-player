# Optimized playback test build

Date: 2026-09-15

## APK

- File: `torrent-player-optimized-playback.apk`
- Package/version: `webtor.app`, 1.4.3 (20), arm64-v8a
- Size: 9,610,656 bytes (9.17 MiB)
- SHA-256: `3285827d9ededfca857214a99c2e1ca881c14a8cf34f166cee6aa337740dec59`
- Release optimization and resource shrinking enabled; signed with the same Android debug certificate as the previous phone installation. This is a local test build.
- Certificate SHA-256: `9c33627be30850a9fd315b374896b5fe229bd8c09c51e0d7a77583cd6d3a38f3`

Install as an update: `adb install -r artifacts/torrent-player-optimized-playback.apk`.

## Changes

- Direct JNI commands replace Android control HTTP/OkHttp. Media streaming compatibility endpoint remains.
- One native observer delivers cancellable UI notifications; metadata, command and final verification changes are retained. Telemetry still uses the existing one-second native sampler.
- Removed bundled fonts and unused anet mobile binaries. UUID remains only as a dependency of WebRTC ICE.
- Read-only Android content URIs expose filename, length, MIME and seek support. Incomplete files use verified torrent reads; completed files open the saved descriptor directly.
- Completion excludes unverified chunks. Playback priorities stay within the requested file's piece bounds. ETA uses milliseconds as expected by the UI.

## Validation

- 13 Go tests pass under the race detector, including three consecutive runs after fixing asynchronous test assumptions; `go vet` passes.
- 13 core and 29 Android app JVM tests pass.
- Debug and locally signed release builds pass, including release lint checks.
- Host JVM CheckJNI exercises the actual Go exports: UTF-8 paths, control responses, descriptor duplication, event delivery, pause/resume, binary reads, seeking, EOF and close.
- A generated H.264/AAC MP4 transfers through a local TCP BitTorrent seed, native destination storage and playback reader; the returned bytes match exactly and decode with FFmpeg.
- HTTP HEAD, partial ranges, suffix ranges, full reads and invalid ranges pass; native reads wait for verified data and cancel on close.
- APK signature and ZIP integrity verified. Both expected native libraries are packaged; no bundled fonts, Node/npm assets, AARs or OkHttp classes remain.

## Device follow-up

The phone was disconnected during this build. Android proxy-descriptor behavior and the user's external player have not been exercised on the new APK. No public-swarm throughput or battery benchmark was run.

Test Play during a partial download, seek forward/back, pause/resume, then play after completion and after reopening the app. If playback still fails, report the player name, video extension, whether the file is complete, and the displayed error.

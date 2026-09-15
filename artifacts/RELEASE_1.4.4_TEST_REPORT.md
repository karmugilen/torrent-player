# Release 1.4.4 verification

Date: 2026-09-15

## APK

- `torrent-player-1.4.4.apk`: 8,797,969 bytes (8.39 MiB).
- Package `webtor.app`; version 1.4.4 (21); arm64-v8a; minSdk 26, targetSdk 34.
- SHA-256: `112bd37f908dd052a67c17dfaa506480bd7438cd5172917e75af5d289f556a68`.
- Signature SHA-256: `9c33627be30850a9fd315b374896b5fe229bd8c09c51e0d7a77583cd6d3a38f3`.
- Same Android debug certificate as the previous public v1.4.2 and phone test APK;
  release optimization and resource shrinking enabled. F-Droid signs separately.
- APK signature, ZIP integrity, version, ABI and packaged dependency notices checked.

## Passed

- 23 Go tests with `-tags=nosqlite,noboltdb -race`; `go vet` passes.
- Tracker parsing/normalization/bounds, invalid cache recovery, conditional 304
  refresh, preservation on HTTP/HTML/oversized-response failures, persisted retry
  deadlines, concurrent cache reads, cancellation, private/public metadata,
  magnet deferral, supplied tracker tiers, lifetime supplement cap, and export/restore
  cycles that keep managed trackers out of persisted torrent metadata and magnets.
- A real local HTTP tracker returns a peer address; the engine connects through
  TCP and retains that connection when the supplemental list changes.
- Existing local TCP media transfer, verified binary playback, seeks, HTTP ranges,
  final completion, pause/resume and storage tests remain green. FFmpeg decodes
  the MP4 returned through the native playback reader.
- Host JVM CheckJNI: native exports, Unicode, descriptor duplication, change events,
  pause/resume, playback reads, seeking and close.
- 13 Android core and 29 Android app unit tests, release lint and APK assembly.
- Live production HTTPS list fetch: 20 valid cached entries, ETag recorded and
  next refresh scheduled 24 hours later. No torrent was added for that smoke test.
- F-Droid 2.4.2 metadata lint and rewritemeta in the buildserver-trixie image.

## Release follow-up

Clean F-Droid source-build and submission status are recorded in
[`docs/FDROID_SUBMISSION.md`](../docs/FDROID_SUBMISSION.md).
Installed the final APK on the connected A015 phone. Reproduced the user's
`tr=DHT` magnet failure as an `unknown scheme` panic in a regression test,
then verified the same supplied magnet loads metadata on the fixed phone build.
Public-swarm speed comparison remains a user test. No guaranteed peer-count,
speed or battery improvement is claimed.

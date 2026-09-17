# App improvement implementation report

Date: 2026-09-16  
Starting commit: `b09c930` (`go-engine`)  
Starting state: existing user changes were present in Android and Go sources, including untracked restore/tracker sources. They were preserved as the implementation baseline.

## Scope

- Stability phases: tracker reporting, startup/restore responsiveness, playback priority ownership, thumbnail lifecycle, truthful stages/telemetry, and release validation.
- Requested workflow: accept and retain magnets before peers/metadata are available; allow file download selection to change from Details without deleting saved bytes.
- Additional milestones are recorded separately in the implementation plan and must not be marked complete without their persistence, cancellation, migration, and acceptance tests.

## Validation record

| Check | Result |
| --- | --- |
| Starting `git diff --check` | Pass |
| Go unit tests | Pass: `go test -count=1 ./...` |
| Go race tests | Pass: `go test -race -count=1 ./...` |
| Go vet | Pass: `go vet ./...` |
| JNI boundary tests | Pass: UTF-8, control, descriptors, events, pause/resume, playback, seeking and close |
| Android unit tests | Pass: `:core:test`, `:app:testDebugUnitTest` (48 tests) |
| Android release lint | Pass: `:app:lintRelease` |
| Android release APK | Pass: optimized release with R8/resource shrinking and local update-compatible signing |
| APK identity/signature/native library | Pass: `webtor.app`, version 1.4.5 (22), matching debug certificate, arm64 `libengine.so` |
| Connected-device installation | Pass: `adb install -r`; existing data retained; activity launched and remained resumed |
| Device behavior matrix | Startup smoke passed; torrent/network behavior awaits user testing with real magnets/files |

## Device context

- ADB device visible before the final build: `A015` (`00121649A004784`).
- Device tests must preserve the user's existing download library and files.

## Implementation notes

### Delivered behavior

- Tracker status now retains logger-bound URL/infohash context, does not leak
  unassociated status between torrents, separates zero-peer replies from errors,
  and identifies original versus supplemental trackers.
- The saved library is published before slow restore work. Status events and
  unrelated commands no longer wait for every metadata restore to finish.
- A valid magnet can be added while offline or without peers. The pending entry
  persists, resumes discovery after restart, and materializes its files once
  metadata arrives. Pause/delete and materialization use the same serialized
  command transition so late work cannot reverse user intent.
- Empty file selection is an explicit stopped state, never false completion.
  File selection can change in Download Details; existing bytes/descriptors are
  retained, initially unselected files get a destination lazily, and the exact
  generated SAF directory is persisted for later files.
- Playback priorities are coordinated per torrent across multiple readers.
  Closing one reader preserves the others; old seek demands are released; pause,
  deselection and removal remain authoritative. Completed deselected files can
  still be opened locally.
- Failed saved-data verification can be retried. Selected-piece telemetry counts
  the actual selected union. Sparse restore and JNI boundary regressions pass.
- Thumbnail work is bounded/coalesced, keyed by URI, protected by invalidation
  generations and a memory-byte budget. Download Details includes collapsed,
  redacted tracker connection information.
- Added stable library sorting/search, batch pause/resume helpers, bounded
  5/15/45-second retry decisions, an optional persistent 1/2/3-slot queue,
  completion notifications with persisted deduplication, and an explicit
  bounded/redacted diagnostics preview and share flow.
- Added global upload/download limits, Any/Wi-Fi/Unmetered policy controls and a
  native payload-only transfer gate. Discovery and magnet metadata remain live;
  automatic network/queue restrictions do not overwrite manual Pause intent.
- Added folder selection through Android's document tree picker and persisted
  exact per-download SAF group directories. File details can focus one selected
  file for earlier download and display verified-byte telemetry.

### Final artifact

- Final file: `artifacts/torrent-player-1.4.5-complete-workflow.apk`
- SHA-256: `1973c9c32afd5e617b282767d96621ef7f1d923d4f70c2c4ede046faaa54904c`
- Signer certificate SHA-256: `9c33627be30850a9fd315b374896b5fe229bd8c09c51e0d7a77583cd6d3a38f3`
- Native library: `lib/arm64-v8a/libengine.so` (21,477,488 bytes before APK compression)

### Device result

Installed on `A015` (`00121649A004784`) with `adb install -r`. Android reported
`Success`; `webtor.app/.MainActivity` became the resumed activity and the process
remained alive during the startup smoke check. OEM graphics/ashmem warnings were
present, with no Java/native crash in the app process.

Real peer availability, force-stop recovery, late metadata arrival, per-file
reselection and playback seeking still require the user's normal device dataset.
The build does not claim network performance percentages from variable public
swarms.

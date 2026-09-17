# Torrent Player: improvement plan and implementation handoff

Date: 2026-09-16. Reviewed checkout: `b09c930`, branch `go-engine`, plus the
user's existing uncommitted changes. Recheck the checkout before implementing.

Expanded after a second design review: sections 10–12 define additional features,
their dependencies, edge cases and separate APK milestones.

Latest explicit user requirements: **accept a magnet with zero connected peers,
keep trying after it is added, and download when it becomes available (F9)**;
**let users uncheck/stop individual files from download details and reselect them
later without losing saved bytes (F10)**. Prioritize these after the stability work.

**This document is a plan. No application code was changed or APK built for this
planning task.** Its purpose is to let a lower-cost coding model work in small,
verifiable steps without repeating the whole investigation.

## 1. Goal and first principles

Make adding, restoring, controlling and playing downloads feel fast while
preserving downloaded data and every existing connection protocol.

The user experiences several different waits:

| Wait | Actual constraint | Useful improvement |
| --- | --- | --- |
| Opening the library | Local state loading and initialization gates | Show saved entries promptly; restore independently of unrelated controls. |
| Getting a magnet's file list | Finding a reachable peer with metadata | Reuse saved metadata, retain existing discovery, explain the current stage. |
| Resuming saved files | Reading and verifying existing bytes | Keep verification asynchronous; optimize only work whose result is provably equivalent. |
| Starting/seeking video | Acquiring verified pieces needed by the player | Give the current read priority; manage startup metadata and old seek priorities correctly. |
| Scrolling and switching screens | UI work, image decoding, storage I/O | Avoid repeated decoding and background preview work; reuse valid frames. |

More peer addresses do not necessarily mean more connected peers or faster
downloads. More simultaneous disk work can also increase waiting. Optimize
measured delays and useful verified throughput, rather than the largest number
in the peer counter. Do not promise percentage gains from this plan.

## 2. Instructions for the implementing model

1. Read this document first, then `git status --short` and the relevant files for
   the current phase. Read applicable `AGENTS.md` instructions if present.
2. Preserve the user's existing changes, including untracked Go sources. Never
   use `git reset --hard`, `git clean`, or restore whole files to HEAD. HEAD alone
   does not contain the latest implementation. Do not include `.mem/` in delivery.
3. Implement required phases 0–6 first. Keep each change bounded and run its
   focused checks before moving on. The additional features in section 11 follow
   as separate APK milestones in section 12, not one large combined patch.
4. Reproduce suspected defects before changing behavior. Where a regression
   passes and disproves a suspicion, record that result and preserve the code.
5. Keep Go/Kotlin models and parsers compatible. Prefer additive optional status
   fields; preserve old saved-library loading and existing lifecycle semantics.
6. Avoid dependency upgrades, architecture rewrites and new background services.
   Keep connection tuning unchanged. F4 adds explicit user-selected transfer
   controls; it does not authorize changing peer-discovery defaults. Introduce a
   small test seam rather than moving `LibrarySession` into a new framework.
7. Run targeted checks during development and full validation before each APK
   milestone. Repeat checks only after relevant changes or failures. Do not spend time
   re-reading the entire repository or running public-swarm benchmarks repeatedly.
8. Update the checklist below with actual commands/results. If interrupted, leave
   the next action and any failing test so another model can resume cheaply.

The immediate delivery is a tested local APK and a concise report. GitHub release
and F-Droid publication are separate delivery work; the F-Droid submission script
is not a prerequisite for compiling or handing over a test APK.

## 3. Current implementation: preserve and build on it

- Android uses `EngineHost` → JNI → Go, with native change notifications. There
  is no Node/npm runtime or Android control server on port 18080. The Go HTTP
  development adapter and media endpoint still exist intentionally.
- `engine-go/trackers.go` already fetches/caches a bounded maintained public list,
  preserves supplied tracker tiers, rejects invalid tracker labels, and adds
  supplements only after public metadata is known. This feature is implemented.
- TCP, uTP, WebRTC, DHT and PEX are enabled in `NewEngineServer`. Android's default
  established-peer limit is 55, with a setting from 8 to 80. Dialing is limited to
  30/s; half-open limits are 24 per torrent and 48 overall. Preserve these values.
- `verification.go` already checks saved data in a cancellable background task
  outside the JNI request deadline, with progress and pause/resume support.
- The working changes already open selected Android files once, check available
  metadata before waiting, and optimize confirmed sparse holes during restore.
- Playback reads verified pieces through a persistent proxy reader; completed
  files open their saved descriptor. Keep both paths and HTTP range support.
- The working changes add seeking windows, tracker status models, one live
  preview frame, and up to three cached completed-video frames.

`HANDOFF.md` contains older historical architecture/configuration below newer
updates. Current code takes precedence. The existing preparation, recovery and
tracker plans describe earlier work; do not redo their completed items.

## 4. Required work, ordered for implementation

### Phase 0 — Capture a baseline and protect existing work

**Files:** the worktree diff, existing Go tests, Android tests and build scripts.

- Record starting commit/status and the existing changed-file list in a local
  implementation report. Inspect untracked source files as well as `git diff`.
- Run the existing Go suite and Android unit suites once. Earlier analysis
  reported successful tests/builds, but that is not proof for a future checkout.
- Establish a repeatable local test using the existing torrent/media fixtures.
  Measure cold library display, metadata-ready, configure acknowledgement,
  verification completion, play-to-first-frame and seek-to-frame separately.
- Use monotonic elapsed time for durations. Instrument only missing boundaries,
  with lightweight local/debug diagnostics; avoid per-piece production logging.
- Use a fixed local seed and fixed data for comparisons. For device results,
  record phone, network, selected bytes, downloaded bytes and warm/cold state.
  Five runs can support a median/range, not a credible p95 claim.

**Done when:** the baseline is recorded; failures are understood rather than
silently attributed to new work. Phone measurements can be marked pending if no
authorized device is available. Do not fabricate them.

### Phase 1 — Make tracker diagnostics accurate

**Files:** `engine-go/tracker_status.go`, `tracker_status_test.go`, `server.go`,
`types.go`; Android `core/.../EngineClient.kt` and its tests.

**Confirmed defect:** `trackerStatusHandler.WithAttrs` forwards attributes only
to the next handler. `noteAnnounceRecord` reads only attributes on the individual
record. In pinned `anacrolix/torrent v1.61.0`,
`client-tracker-announcer.go:singleAnnounceAttempter` sets `url` and
`short infohash` with `logger.With(...)`, then logs `announced` with `resp`/`err`.
The capture handler therefore misses identity on this real HTTP/UDP log path.
The existing test puts every attribute directly on the record and misses it.

1. Add a failing regression using a real `slog.Logger.With(...)` and the same
   announce shape. Include chained attributes/groups and two torrent identities.
2. Preserve bound attributes and group context in handler copies, without sharing
   mutable slices between loggers. Parse the relevant resolved attributes from
   both bound context and the record; preserve downstream logging behavior.
3. Do not use `trackerStatus[""]` as another torrent's status fallback. If a
   callback cannot be assigned to a torrent, leave that torrent's status unknown.
   Verify the pinned callback identity format rather than guessing it.
4. Correct origin classification: no original trackers means added trackers are
   supplemental, not original. Test a trackerless public torrent after additions.
5. Test that actual attached supplements appear once alongside original tiers.
   Do not merge arbitrary unassociated log URLs into every torrent's view.
6. Keep a tracker response reporting zero peers distinct from an announce error.
   A WebSocket connection alone is not evidence of a successful peer connection.

**Done when:** HTTP/UDP and WebSocket status tests pass, two torrents cannot
borrow each other's results, and supplied trackers/privacy/caps remain unchanged.
This phase improves reporting; do not claim that it increases peer connections.

### Phase 2 — Keep startup and resume controls responsive

**Files:** Android `LibrarySession.kt`, `UiModels.kt`, `DownloadStorage.kt`,
`EngineHost.kt`; focused coordinator/session tests as needed.

**Confirmed blocking structure:** `start()` completes `initialized` only after
`restoreAll`. Both `launchCommand` and `eventLoop` wait for `initialized`.
`restoreAll` restores entries sequentially under the shared command mutex, and
`restoreEntry` can wait up to 30 seconds for metadata. This can delay controls
and status for unrelated entries even though the screen is already visible.

1. Separate “library loaded / commands can inspect state” from “all eligible
   entries restored.” Complete the former once saved state is safely published.
   Commands requiring native operations still await engine readiness explicitly.
2. Start status consumption once the engine is ready; it must not wait for every
   restore. Keep `restoring` as display state, not a global command barrier.
3. Move slow restore I/O outside the global command lock. Start conservatively
   with one restore worker, not unbounded parallel restores. Use per-entry jobs
   or a small coordinator to prevent two restores of the same entry.
4. Capture entry generation and user intent under a short lock; perform metadata
   and descriptor work outside it; recheck identity/generation before publishing.
   Pause/delete/stop must invalidate queued or in-flight restore intent promptly.
5. Preserve cleanup of newly created files and duplicated descriptors. A stale
   restore must not remove an engine record now owned by a newer operation;
   the engine may deduplicate adds for the same torrent. Track ownership and
   serialize same-entry cleanup/retry, not just the final UI assignment.
6. Completed files should be playable without waiting for unrelated restoration.
   Paused entries must stay paused after relaunch. A startup pause should cancel
   that entry's queued restore without allowing it to become active later.
7. Preserve cancellation (`CancellationException` must propagate), notification
   stop intent, persistence ordering and generation checks. Use monotonic time
   for metadata deadlines when touching `waitReady`.

**Tests:** hold torrent A's metadata/storage step with a fake dependency, then
exercise pause/delete on A and play/status on B; release A and prove it cannot
resurrect or overwrite newer state. Cover pause → resume, stop-all, failed restore
→ retry, duplicate add, and shutdown during descriptor opening. Test actual
coordinator behavior with controlled suspension, not only boolean helper methods.

**Done when:** unrelated controls/status progress while A remains blocked;
ordinary commands acknowledge promptly without waiting on A's timeout. Measure
UI acknowledgement separately from eventual disk/network completion.

### Phase 3 — Correct playback priority ownership before tuning buffers

**Files:** `engine-go/playback.go`, `playback_test.go`, `server.go`,
`verification.go`; Android `PlaybackProvider.kt` only if lifecycle fixes require it.

**Observed hazards to reproduce:** `/play` boosts the file head/tail, but
`OpenPlayback` immediately updates a window near offset zero and can demote the
tail outside it. Each reader writes torrent-global explicit piece priorities;
closing one resets spans to `Normal`, potentially affecting another reader or
leaving priority on deselected files. Existing `testPiecePriority` derives an
answer from reader bookkeeping rather than reading the engine's actual priority.

1. Write regressions against real engine state/transfer behavior. The pinned
   library exposes `Piece.State().Priority`; use controlled incomplete fixtures
   where verification/data-download state does not mask effective priority.
   Retain the existing actual verified-read/local-peer tests.
2. Keep the urgent current-read window and bounded readahead. Retain startup
   head/tail demand long enough to fetch container metadata; release each boost
   once its data is verified or its playback session closes. Keep tail startup
   priority below the urgent current read. Do not blindly boost the whole file.
3. Use one owner of explicit playback priorities per torrent, with demands from
   active playback handles. The effective demand is the maximum across active
   handles and startup ranges. Closing one handle removes only its demand.
   Keep this small and local; do not replace anacrolix's request scheduler.
4. Where no handle needs an explicit boost, clear it to `None` so existing file
   selection priorities supply the base priority. Do not reset everything to
   `Normal`. Define lock ordering; never hold the coordinator lock during reads
   or while waiting for a handle to close.
5. Update/demote the full previous demand set, including evicted or partially
   overlapping seek hotspots. Prove old high-priority pieces do not accumulate.
   Apply changes only where demand changed, instead of resetting every piece
   priority for each small proxy read.
6. Pause/check/remove must retain authority over payload transfers. Enforce
   playback preconditions at the native entry point as well as the UI, so an old
   content URI cannot bypass current verification/selection state. Release reader
   demands safely on deselection/removal and cancel blocked reads on close.
7. Preserve exact byte ranges, EOF semantics, verified data, completed-descriptor
   playback and HTTP behavior. Never return zero-filled or unverified bytes to
   make a player appear to start faster. Keep current buffer sizes until measured.

**Tests:** head/tail startup on a file longer than the readahead window; repeated
forward/backward seeks with partially overlapping hotspots; two readers then
close one; deselect/pause/resume/remove during playback; close a blocked read;
completed media; HTTP range responses; MP4 with metadata at the tail if a fixture
can be generated with the existing test tools. Validate that fixture's layout.

**Done when:** real priority/read regressions and Go race checks pass. A phone
test must still check player startup, seeking and closing, because host tests do
not prove Android proxy callback behavior. Report pending device checks honestly.

### Phase 4 — Spend less CPU and I/O on thumbnails

**Files:** Android `ThumbnailRepository.kt`, `LibraryScreen.kt`,
`TorrentFilesScreen.kt`, `Formatters.kt`; cache/scheduling tests.

**Observed:** both preview effects loop every four seconds while downloading.
Unlike the details telemetry effect, these effects are not lifecycle-gated.
The repository serializes by key but does not coalesce duplicate live extraction;
cache identity omits URI; invalidation can race an extraction and be overwritten.
`withTimeoutOrNull` around blocking decoder calls is not a hard native timeout.

1. Gate preview work to the screen's STARTED lifecycle and relevant visible rows.
   Keep already rendered frames when stopped; suspend refresh while checking,
   paused, removed or without new useful progress. Coalesce concurrent requests
   for one preview key into one extraction shared by callers.
2. Keep live previews and completed three-frame previews. Add a minimum refresh
   interval plus a meaningful verified-progress change; back off repeated empty
   results. Total torrent progress does not identify a decodable timestamp,
   because torrent pieces arrive out of order. Do not launch extra network reads
   to improve a thumbnail; keep a cached frame or placeholder when unavailable.
3. Include stable file identity (for example a hash of the saved URI) and an
   explicit invalidation generation in cache handling. Do not use the download's
   general command generation, which changes on ordinary pause/resume.
4. Check generation again before publishing an extraction. Invalidation/delete
   must prevent old work from recreating memory/disk cache entries. A transient
   `.failed` result needs bounded retry or identity-based expiry, not permanent
   failure for a file that later becomes readable.
5. Keep extraction concurrency bounded, including work whose coroutine has been
   cancelled but whose native decoder still runs. Do not start a replacement
   worker on every timeout or claim cooperative timeout kills native decoding.
   Keep release/close in `finally`; do not recycle bitmaps still held by Compose.
6. Bound memory by decoded bitmap bytes (use a modest initial budget, e.g. 24 MiB),
   not only 24 entries. Preserve disk limits. Move directory scanning/deletion
   during cache invalidation off the main thread. Clean up per-key coordination
   entries after work ends; avoid an ever-growing lock map.

**Tests:** concurrent consumers share work; no new extraction when hidden or
without progress; live → complete refresh; URI replacement at the same length;
invalidate/delete during extraction; bounded failure retry; memory eviction.
Use a fake extractor for scheduling tests and real media for device validation.

**Done when:** live/final previews still work, background preview decoding stops,
scrolling does not trigger repeated work for identical data, and stale extraction
cannot repopulate an invalidated cache.

### Phase 5 — Explain waiting accurately and validate restore optimization

**Files:** Android `PrepareScreen.kt`, `UiModels.kt`, `DownloadStorage.kt`,
`LibrarySession.kt`, `TorrentFilesScreen.kt`; Go `restore_hash.go`,
`restore_hash_test.go`, `sparse_unix.go`, `zero_hash.go`, `verification.go`.

1. Derive a small transient display stage from current operations: Starting,
   Finding peers, Fetching file list, Opening saved files, Checking saved data,
   Downloading, Paused, Complete or actionable error. Avoid a second independent
   persisted state machine. Existing checking progress should be reused.
2. Keep startup informational until an actual error occurs. Keep pause/resume
   and retry states clear; slow metadata with no reachable peer is not an engine
   crash. Do not display guessed progress/ETAs for work that cannot be measured.
3. The tracker list is parsed in Kotlin but currently has no tracker section in
   `TorrentFilesScreen`. Add a collapsed connection-details section using the
   corrected status. Label connected peers separately from tracker-reported
   addresses, which can overlap. Keep advanced detail out of the main flow.
4. Display safe tracker host/protocol and status. Hide userinfo, sensitive path
   segments, query credentials and any echoed credentials in raw error messages.
   This is display redaction only: preserve the full original tracker internally.
5. Validate sparse hashing against full hashing rather than adding another
   shortcut. Cover dense, all-hole, mixed, corrupt, zero-length, short final,
   cross-file pieces, unsupported hole seeking, and v2/hybrid fallback. A hash
   mismatch must remain an incomplete piece, not a verification transport error.
6. Add a regression for a file shortened after descriptor attachment. `SEEK_DATA`
   returning no more data does not alone prove the entire expected extent exists.
   Use the shortcut only when the entire piece span is backed by valid file
   lengths and confirmed holes; fall back to normal reads when uncertain. Never
   treat missing bytes beyond EOF as verified zero content.
7. Verify that the restore-only hash wrapper is removed before peer-data hashing
   resumes, including error/cancel paths. Keep v2/hybrid on the library path.
   If profiling shows meaningful churn, reuse a bounded hash buffer and bound
   the zero-hash cache; do not add complexity without evidence.
8. Correct selected-piece telemetry before building new buffer displays on it.
   `handlePieces` currently assigns `b.Selected = b.Total` for every bucket.
   Count the union of pieces overlapping selected files, including shared boundary
   pieces once; preserve torrent-wide verified counts with clearly defined labels.
   Test a multi-file torrent with one selected file and a shared boundary.

**Done when:** stages reflect actual operations; tracker UI uses corrected data;
sparse/full paths agree on verification results; restart preserves valid bytes
and downloads only missing/corrupt data. Device speedup is measured, not inferred
from the existing all-hole microbenchmark's effective throughput.

### Phase 6 — Validate and deliver the APK

Run from the repository root, using subshells so directory changes do not leak:

```bash
(cd engine-go && go test ./...)
(cd engine-go && go test -race ./...)
(cd engine-go && go vet ./...)
JAVA_HOME=/usr/lib/jvm/java-21-openjdk bash scripts/test-jni.sh
(cd android && JAVA_HOME=/usr/lib/jvm/java-21-openjdk ./gradlew \
  :core:test :app:testDebugUnitTest :app:lintRelease :app:assembleRelease \
  -Pwebtor.localSigning=true)
git diff --check
```

Use that Java path only if installed; otherwise select an existing compatible
JDK. Gradle already builds the Go native library; no separate npm or Node step.
Use focused `go test -run ...` and Android test filters between phases.

Optional benchmark after relevant restore changes:

```bash
(cd engine-go && go test -run '^$' -bench BenchmarkSparseVersusDenseSelfHash -benchmem)
```

Before packaging, check the installed/local signing identity. The localSigning
flag selects the debug signing configuration only when production signing env
variables are absent. Never print passwords or credentials. Use the compatible
existing key for an update; never uninstall the user's app to bypass a mismatch.

Locate the actual APK under `android/app/build/outputs/apk/release/`, verify its
application ID/version/signature/arm64 library, copy it to a clearly named file
under `artifacts/`, and record its SHA-256. Do not overwrite a published version's
artifact. A development APK can retain the current version with a descriptive
filename; coordinate a versionCode increase before an actual new public release.

If the user has requested installation and an authorized device is connected,
use `adb install -r <verified-apk>`. Otherwise provide the APK path. Device tests
use disposable fixtures, not edits/deletion of the user's existing downloads.

## 5. Device acceptance matrix

| Scenario | Required result |
| --- | --- |
| Cold launch, several incomplete downloads | Saved entries appear; a slow restore does not freeze unrelated controls. |
| Magnet with valid trackers plus invalid labels | Valid trackers retained; bad labels ignored; no engine error. |
| Add magnet with zero peers, then peers appear | Entry is saved immediately; metadata and download start later without re-adding. |
| Uncheck/recheck a file in download details | That file's download intent changes; other files continue; saved bytes survive. |
| Uncheck every file, then relaunch | No files selected remains explicit; no false completion or unintended resume. |
| Offline launch or failed tracker refresh | Cached library/trackers available; clear state; working completed playback. |
| Pause → force-stop → reopen | Entry remains paused; saved bytes intact. |
| Download → force-stop/reboot → resume | Checking shows progress; verification finishes without JNI deadline failure. |
| Pause/remove while checking | Prompt acknowledgement; no stale resume or blocked shutdown. |
| Corrupt/truncated/missing saved file | Correct verification/recovery error; no false completion or silent deletion. |
| Play during download; seek forward/back | Verified media loads; seeks eventually fill; no premature EOF. |
| Multiple player descriptors; close one | Other handle remains usable; obsolete boosts are released. |
| Complete file → reopen app → Play | Saved media plays without unrelated restoration blocking it. |
| Scroll, background/foreground, delete previewed entry | Smooth cached previews; no background decode loop or stale preview reappearance. |
| Wi-Fi/mobile/offline transition | Existing reconnection behavior preserved; no forced peer resets added. |

Run long-file checks on an authorized disposable dataset large enough to exceed
the old 60-second verification boundary on that device, or use controlled slow
storage in tests. Do not assume a tiny test fixture exercises that regression.

## 6. Non-negotiable invariants

- User pause/stop/delete intent wins over late restore, metadata and status work.
- Existing files survive retry, force-stop and app updates. Only explicit erase
  actions delete user media. Descriptor ownership/cleanup remains deterministic.
- Saved length/progress is not proof of verification. Preserve piece checking,
  shared file boundaries and verified-only incomplete playback.
- Original valid tracker tiers and credentials are used; public tracker cache
  remains bounded/background-only; private torrents receive no supplements.
- Preserve WebRTC/WebSocket, DHT, PEX, TCP and uTP. Do not increase announce or
  dial rates, reintroduce control polling, or replace the native engine.
- Retain notifications, selected downloads, themes, search, live previews,
  completed previews, file details, delete-keep/delete-erase and media-player
  compatibility. Optimize optional work without silently removing features.

## 7. Completion checklist for the coding model

- [ ] Phase 0: baseline and starting worktree recorded.
- [ ] Phase 1: real logger-context tracker regression fixed.
- [ ] Phase 2: startup/restore command and generation races covered.
- [ ] Phase 3: actual playback priorities and multiple readers covered.
- [ ] Phase 4: lifecycle-aware previews, deduplication and invalidation covered.
- [ ] Phase 5: clear stages, tracker details and restore safety covered.
- [ ] Phase 6: full checks pass; APK identity/signature/hash recorded.
- [ ] Device checks recorded as passed, failed or not run, with reasons.
- [ ] Final response includes APK link, concrete changes and remaining limits.

For each phase record: files changed, reproduced failure, fix, command/result and
next step. Put longer evidence in `artifacts/APP_IMPROVEMENT_TEST_REPORT.md` when
implementing. Do not mark a phase complete merely because the code compiles.

## 8. Measuring whether the work helped

Compare before/after on the same fixture/device. Record library display latency,
command acknowledgement while another restore is blocked, verification duration,
first-frame and seek latency, preview extraction count, peak memory and CPU.
Report regressions as well as improvements. Public swarm speed varies with peer
availability, so a different day's torrent speed does not establish a code gain.

If a change adds complexity without improving the measured problem, simplify that
change while preserving the user's pre-existing work. Do not expand networking
limits to disguise slow disk, verification or UI behavior.

## 9. Suggested prompt after switching models

> Read docs/APP_IMPROVEMENT_IMPLEMENTATION_PLAN.md and implement required phases
> 0–6 in small steps. Preserve all my existing edits and features. Reproduce the
> identified bugs, run focused tests, then the final validation and build a signed
> update-compatible APK. Give me the APK first and report any device checks you
> could not run. Update the checklist and leave a short handoff if interrupted.
> Deliver the stability APK before beginning additional feature milestones.

For the feature work after that APK:

> Continue docs/APP_IMPROVEMENT_IMPLEMENTATION_PLAN.md with the next unfinished
> milestone in section 12. Implement its feature IDs from section 11, including
> their acceptance checks. Preserve existing downloads and defaults. Build and
> give me that milestone's APK and update the checklist before starting another
> milestone. Follow the shared pause/network decision rules in section 10.

## 10. Second design review: resolve these traps before adding features

**A. A responsive screen is not the same as a resumed download.** Report separate
durations for showing the library, accepting Resume, finishing verification and
receiving useful data. Do not hide the checking stage to claim faster resume.
When several torrents restore, measure simultaneous verification I/O. If it hurts
controls/playback, bound verification work with a cancellable shared scheduler;
a paused torrent must not hold the only permit indefinitely. This is a measured
follow-up to Phase 2, not a reason to rewrite the engine preemptively.

**B. Fixing tracker status will not create peers.** The cached tracker feature is
already implemented; supplements are attached after public metadata is known.
Do not claim those supplements speed up the initial file-list fetch for every
magnet. Distinguish tracker replies, candidate addresses, established connections
and actual data transfer. Keep private-torrent protections and existing budgets.

**C. User intent must be separate from automatic restrictions.** Before F2/F4/F9/F10,
define one small decision function shared by controls, restoration, notifications
and automatic retry. Conceptually it takes persisted user run intent, entry error,
engine readiness, storage access and network policy, and returns allowed operations
plus a visible reason. **F3 download-slot queue eligibility is withdrawn** — eligible
torrents may transfer simultaneously. Do not let each feature call resume
independently. Preserve additional reasons even when showing only one.

| Condition | Required behavior |
| --- | --- |
| Deleting/stopping or user-paused | No automatic resume; old jobs cannot reverse intent. |
| Missing storage access or a fatal error | Action required; no automatic destructive repair. |
| Engine unavailable | Starting/retry state; preserve user intent and saved files. |
| Magnet accepted, metadata not yet available | Keep the entry; metadata discovery continues when permitted; no payload without files/storage. |
| Metadata ready, explicitly no files selected | No payload; retain files/entry and wait for selection; not Complete. |
| Network policy blocks transfers | Show the policy reason; completed local playback remains available. |
| Eligible, saved data needs checking | Check saved data; keep payload gated until safe. |
| Eligible and verified/configured | Download using existing peer behavior (no slot limit). |

Migrate old records conservatively from `paused` and `serviceSuppressed`; avoid
multiple competing persisted booleans. An automatic network pause must not become
a permanent user pause. Stop-all records manual intent. A network reconnect,
notification, retry or Play request must pass the same decision rules. Keep
ordinary Pause immediate; it should not require a confirmation dialog. Clear
legacy `downloadQueueMode` / `downloadQueueOrder` prefs on upgrade so a former
slot hold cannot leave native selection empty while the user still has saved
selected files — restore/configure re-applies saved selection without unpausing
user-paused entries.

**D. Restore must not destroy useful work.** Retain configured records across
process death and preserve in-process verified state. Do not drop and re-add
torrents merely to change transfer order (slot reordering is gone with F3). A
genuine process death still uses safe restore verification.

**E. A player read offset is not a playback timestamp.** External players may
probe the file tail for metadata. Expose verified byte availability for a selected
read region, and identify that region honestly. Do not label it “seconds buffered”
or promise resume-at-last-watched-time without a tested player integration.
First-frame timing needs player observation/instrumentation; JNI open success
alone does not establish that a frame appeared.

**F. Storage repair is a data operation.** Current `AttachDescriptors` sizes files
with `Truncate`. Never pass an arbitrary replacement file into this path before
identity/size validation and an explicit repair decision. Matching filenames or
lengths are only candidates, not proof. Work on new mappings transactionally and
retain the original mapping until validation succeeds.

**G. Keep feature cost visible.** Each feature below has a bounded first version.
Do not add accounts, analytics services, a database migration, a new media player
or a scheduling framework just to deliver a small UI feature. Cost labels describe
relative implementation/risk, not guaranteed hours. Measure regressions after
each APK milestone instead of hiding many changes in one release.

## 11. Additional features to implement after the stability APK

All Kotlin paths below are under `android/app/src/main/java/webtor/app/` unless
otherwise specified. Reuse the existing JNI/event channel and foreground service.

| ID | Feature | Cost | Prerequisites |
| --- | --- | --- | --- |
| F1 | Library filters, sorting and batch controls | Small–medium | Phase 2 |
| F2 | Actionable stalls and bounded automatic retry | Medium | Phases 1/2/5; shared intent rules |
| F3 | Download slot queue (removed 2026-09-16) | — | Withdrawn; see F3 section |
| F4 | Network policy and upload/download caps | Medium–large | F2 decision integration |
| F5 | Folder selection, space awareness and storage repair | Large | Phases 2/5 |
| F6 | Download-first file focus and verified read status | Medium | Phase 3; F10; selected-piece telemetry |
| F7 | Completion notifications and reliable action routing | Medium | Phase 2; shared intent rules; F10 completion semantics |
| F8 | User-triggered, redacted diagnostic export | Small–medium | Phases 1/5 |
| F9 | Add now with zero peers; download when available | Medium–large | Phase 2; F2; explicit pending-entry persistence |
| F10 | Change individual file downloads from details | Large | Phases 2/3/5; F9 persistence distinctions |

### F1 — Find and control downloads without opening each one

**Implementation:** `LibraryScreen.kt`, `UiModels.kt`, `LibrarySession.kt`,
`MainActivity.kt`, existing/new pure list projection tests.

- Keep current search. Add filters: All, Active (preparing/checking/downloading),
  Paused, Complete and Needs attention (no Queued slot state after F3 removal). Define precedence
  once so a checking item with old 100% progress is not listed as complete.
- Add stable sorts: Newest (default), Name, Size and Progress. Use natural ordering
  for numbered episode names; break ties by stable key. Preserve filter/query,
  scroll position and sort across rotation/navigation; persist sort preference.
- Add explicit selection mode with Pause selected and Resume selected. Show how
  many entries are selected, support clear/select-all-filtered, and exclude entries
  that become deleted. Reuse per-entry command/generation handling from Phase 2.
- Avoid bulk permanent delete in the first version. Keep current individual
  delete-keep/delete-erase flows. Report partial batch failure per entry; one bad
  item must not cancel every other command or falsely report full success.

**Acceptance:** combinations of query/filter/sort give stable results; filtering
does not change download order; bulk Pause followed by reconnect stays paused;
late completion/removal while selecting does not target another item. Check large
text, content descriptions and selection announcements as part of this UI work.

### F2 — Explain stalls and recover from temporary failures

**Implementation:** `LibrarySession.kt`, `PrepareScreen.kt`, `UiModels.kt`,
`EngineClient.kt`, structured error fields in `engine-go/types.go`/handlers.

- Distinguish offline, waiting for metadata, peers connected but not supplying
  data, checking files, unavailable storage and engine failure. A tracker error
  alone must not make a working download fail. Zero speed alone is not a crash.
- Use structured error categories where possible; do not scatter comparisons
  against human-readable error text. Show a relevant action such as Retry,
  Reconnect folder, Open details or Cancel preparation.
- Retry only failed transient operations. Start with at most three retries per
  uninterrupted failure episode, at 5/15/45 seconds with bounded jitter. A relevant
  network recovery may advance one scheduled retry, not spawn another job.
  Deduplicate by entry/operation/generation and persist explicit user intent.
- Missing peers/metadata without a command failure is a waiting state: let the
  library's own tracker/DHT schedule continue. Do not re-add the torrent, force
  announces or restart the whole engine to retry metadata. Offer manual Retry
  after an exhausted wait and retain cached metadata where available.
- F9 entries remain saved and eligible for normal discovery after the old
  preparation-screen timeout. The three-retry limit applies to failed transient
  commands, not to how long a peerless torrent is allowed to stay in the library.
- Cancel retries on pause/remove/stop; resume only if current policy permits.
  Denied permission, invalid magnet and no space require user action. Do not
  schedule wakeups to bypass a force-stop. Explicit app reopening uses restore.

**Acceptance:** test offline → online, rapid reconnects, cancellation at every
backoff boundary, three failures then stable action-required state, permission
failure without retry, and a slow but working download without repeated resets.

### F3 — Removed by user request (2026-09-16)

Remove the concurrency setting, slot scheduler, queue order and Move up/Move down/
Download next controls. Normal eligible downloads can run together. Old queue
preferences must no longer restrict transfers after upgrading. Preserve manual
pause, file selection, network restrictions and speed limits. Earlier references
to F3, payload slots and queue promotion in this plan are superseded by this
decision. Internal cancellable restore work is separate and remains necessary.

See `docs/PIECES_HEAT_GRID_UI.md` for the active implementation and validation plan.

### F4 — Data controls with explicit, honest scope

**Implementation:** `SettingsScreen.kt`, `LibrarySession.kt`, `WebtorApp.kt`,
one process-owned network observer; Go settings DTOs/handlers and rate limiters.

- Default to Any network. Add Wi-Fi only and Unmetered only as distinct policies;
  Wi-Fi does not necessarily mean unmetered. Observe capabilities through Android
  network callbacks, include VPN/captive-portal cases, and clean up callbacks.
  [Android network-state guidance](https://developer.android.com/develop/connectivity/network-ops/reading-network-state)
- Label the setting as a **transfer policy**: it gates torrent payload uploads
  and downloads. Tracker/DHT/discovery traffic may still occur with the existing
  engine. Do not claim zero mobile-data use or a network firewall; that would
  require separately controlling all engine sockets, trackers and fetches.
- Apply the policy before starting new payload transfers and after network
  changes. Show Waiting for Wi-Fi/Unmetered network, and auto-resume only when
  user intent still permits. Completed local media remains playable. Do not use
  a frequent connectivity timer or add an independent resume path.
- Add global upload/download caps, default Unlimited. Use validated KiB/s input
  with explicit units; zero means Unlimited in the UI and must be mapped safely.
  Pinned anacrolix `ClientConfig` already has `UploadRateLimiter` and
  `DownloadRateLimiter`. Retain distinct limiter references and update them
  safely; do not accidentally share the library's default limiter instance or
  rebuild the client. Keep burst sizes large enough for library requests.
- Verify TCP/uTP/WebRTC paths honor the limiter; label any unsupported scope.
  Protocol overhead and bursts mean this is not an exact mobile-billing cap.
  Do not add schedules, VPN binding or per-torrent caps in this first version.

**Acceptance:** policy changes while checking, paused and playing; metered Wi-Fi;
VPN transitions; offline flapping; persisted limits after restart; both upload
and download caps measured with local transfers after allowing initial bursts.

### F5 — Choose storage and recover lost access safely

**Implementation:** `DownloadStorage.kt`, `LibrarySession.kt`, `MainActivity.kt`,
`SettingsScreen.kt`, add/download UI; Go storage integration regressions.

- Keep Downloads/Webtor as the default. Offer Choose folder for new downloads.
  `createFiles(entry, tree)` already has a tree path, but current session callers
  pass `null`; reuse it and audit all restore/delete/play paths before enabling it.
  Persist the selected destination with the entry, not only as a global setting.
- Use the system folder picker and the offered persistable URI permissions.
  Handle revoked/moved documents and platform-restricted directories. Keep the
  existing MediaStore default path available; do not request broad filesystem
  permission to bypass picker restrictions.
  [Android document and folder access](https://developer.android.com/training/data-storage/shared/documents-files)
- Validate random-access read/write and sizing capability with a disposable file
  before accepting a destination; reject unsupported providers with an actionable
  message. Never run destructive capability probes on the user's existing media.
  Test the folder path on Android 8/9 as well as the MediaStore path on Android 10+.
- Add Reconnect folder/Locate saved files on storage failures. Match candidate
  paths/indexes and validate descriptor sizes read-only before proposing the new
  mapping. Never attach/truncate a larger unrelated file. After the user chooses
  the repair, verify pieces and keep only valid bytes; cancel preserves old state.
- Show selected bytes, known remaining work and destination free space when
  available. Do not equate logical preallocated length with allocated disk space,
  and do not treat unavailable provider free space as zero. Recheck before new
  writes/retries; no-space failures pause safely and never delete other downloads.
- Limit the first version to destinations for new downloads and relinking
  existing files. Moving active downloads to another volume is a later feature.

**Acceptance:** persisted access after reboot; denied/revoked grant; removed SD
card; provider lacking seeking; no-space during download; cancelled repair;
same-name wrong file; multi-file torrent boundaries; delete-keep and delete-erase
stay confined to the correct entry. No broad directory cleanup during repair.

### F6 — Prioritize one file and explain playback availability

**Implementation:** `TorrentFilesScreen.kt`, `LibrarySession.kt`, JNI client/DTOs,
`engine-go/playback.go`, `server.go`, selected-piece telemetry.

- Add Download this file first on an already selected file, with Clear priority.
  Persist at most one focused file per torrent. Other selected files stay selected
  and continue normal downloading; removal/deselection clears invalid focus.
- Integrate focus into the single priority owner from Phase 3. Urgent active reads
  outrank background focus. Clearing focus or closing a reader restores remaining
  demands correctly. Do not add another loop that writes piece priorities.
- Show selected-file verified progress and, when a playback handle is active,
  contiguous verified bytes from its latest read request. With multiple handles,
  identify the region or show an aggregate without pretending there is one cursor.
- Before Play, show Checking saved data, Getting playback data, Downloaded locally
  or a measured byte count. Keep Play available under existing safe conditions;
  a readiness hint is not a guarantee of codec support or instant first frame.
- Bound/cap range computation and update via existing events. Do not expand every
  piece on every UI frame, interpret torrent-wide progress as contiguous data, or
  infer watched episodes from opening a file. Fix shared-boundary counting first.

**Acceptance:** focus changes preserve selection; active reads win; focus survives
restart; sparse out-of-order pieces cannot produce a false contiguous buffer;
tail metadata probes are labelled honestly; fully downloaded files play offline.

### F7 — Useful completion notifications without duplicate alerts

**Implementation:** `PlayService.kt`, `LibrarySession.kt`, `MainActivity.kt`,
`DownloadStorage.kt`; notification action/state tests.

- Keep the existing ongoing notification and pause/resume/stop controls. Add one
  completion notification when the selected download becomes verified complete,
  with Open details and Play when an appropriate selected media file exists.
- Route actions through a stable entry key, reloading current state before acting.
  A deleted/replaced entry must not act on a stale engine ID. Rapid repeated taps
  use the same command deduplication as the main UI.
- Persist a completion acknowledgement/version to suppress duplicate alerts after
  restore. Existing completed downloads are marked as already acknowledged during
  migration. A deliberate new download/selection can start a new completion cycle.
- Handle notification permission denial without breaking in-app controls. Request
  notification permission in context and respect the user's choice.
  [Android notification permission](https://developer.android.com/develop/ui/compose/notifications/notification-permission)
- Do not add a service per torrent, an auto-launching player, or an alarm to bypass
  platform process restrictions. Group multiple completions without alert storms.

**Acceptance:** exactly one normal completion alert; restart/recheck does not
duplicate it; deleted entry action is harmless; permission denied;
while notification actions preserve explicit pause/stop intent.

### F8 — Export enough diagnostics to reproduce failures

**Implementation:** `SettingsScreen.kt`, `TorrentFilesScreen.kt`, session/engine
status DTOs, a small export utility with redaction tests.

- Add Export diagnostics for all entries or a selected download. Generate only
  on request, offer a preview and save/share through an explicit user action.
- Include app/build/Android versions, stage/error categories, durations, checking
  counts, tracker success/error counts, connected peer counts, transfer-policy reason,
  and bounded recent state transitions. Record no raw stream/piece payloads.
- Use an allowlisted schema, not a dump of preferences/logcat/library JSON. Replace
  entry identifiers with report-local labels; omit magnets, infohashes, titles,
  URIs, file paths, peer addresses, tracker passkeys, tokens and raw errors that
  may echo any of these. Summarize network type without SSIDs or local addresses.
- Keep a bounded in-memory event history (initial cap: 100 transitions) and bound
  export size. Do not write a disk log every second or upload reports automatically.
- Test malicious/unusual URLs and error strings containing example secrets.
  A report should still explain the failure stage after redaction.

**Acceptance:** offline export works; no network request occurs; sample secrets
are absent; exported format/version and app identity are present; another person
can distinguish checking timeout, unavailable peers and denied storage access.

### F9 — Add a magnet immediately, even when no peers connect

**User-visible contract:** a valid magnet can be added to Downloads before its
file list is known. Show Waiting for peers or Fetching file list; keep trying
through the existing discovery engine. When metadata arrives, prepare the saved
destination and start downloading according to recorded intent and current policy.
The user must not paste/add it again just because peers appeared later.

**Implementation:** `AddSheet.kt`, `PrepareScreen.kt`, `LibrarySession.kt`,
`DownloadStorage.kt`, `UiModels.kt`, `Formatters.kt`, and affected Go add/status
contracts. Reuse native metadata events and existing `prepare=true` behavior.

**Current blockers:** `commitPreparedDownload` rejects `torrent.ready != true`;
`parseEntry` requires nonempty files and selection; `complete` uses `all` over
selected files, which is true for an empty set; startup skips entries without
selected file URIs. A button-only change would create lost or falsely complete
downloads. Fix the entire persisted lifecycle, not just the add screen.

1. Validate the supported magnet/infohash locally and persist a real pending
   entry before network success. Use a stable normalized torrent identity for
   deduplication, preserving valid supplied trackers and the original source.
   Use the magnet display name or a short identity as a temporary title.
2. Add an explicit persisted distinction between Awaiting metadata and Metadata
   ready. Empty files are valid only for the former; an explicitly empty selection
   after metadata is a different F10 state. Keep existing entries backward
   compatible and reject malformed records without rejecting valid pending ones.
   Do not invent a zero-byte placeholder file or mark an unknown total as 100%.
3. The add action explains its default: Download when ready uses the current
   default file policy (video files if present, otherwise all). Persist that policy
   separately from a known explicit selection. Also offer Choose files when ready
   for users who want metadata discovery only; it waits for their selection instead
   of downloading automatically. Keep this inline, without a blocking extra dialog.
4. Once persisted, hand ownership from the add/preparation draft to the library.
   Closing the sheet cancels only an uncommitted draft; it must not remove the
   saved pending torrent. Re-adding the same hash opens the existing entry,
   preserving its intent, selection and destination rather than duplicating files.
5. Keep metadata exchange active for a runnable pending entry while payload stays
   disabled until selected storage descriptors exist. The current Go add path
   already separates metadata exchange from payload. Preserve original trackers,
   DHT/PEX/WebRTC behavior and defer public supplements until metadata confirms
   public status. No speculative public announce or aggressive retry loop.
6. Consume metadata-ready events outside the global command mutex. Recheck entry
   generation/intent; atomically save real metadata, files and chosen default or
   explicit selection; then prepare storage once and schedule transfer. Persist
   intermediate state so a crash cannot lose the magnet or orphan a second set
   of files. Show actionable destination/space errors instead of deleting the entry.
7. On relaunch, restore metadata-pending entries from their saved magnet even
   though they have no file URIs. For ready entries reuse saved metadata. Pause,
   Stop-all and Delete cancel pending activation; a late metadata event must not
   turn those entries back on. Resume continues discovery from the same entry.
8. Remove the 90-second preparation wait as a lifetime/error limit on committed
   pending entries. A UI wait may end, but the library item remains. After normal
   app close, use the existing permitted background lifecycle; after force-stop,
   resume normal handling when the user reopens the app. Do not promise downloading
   while Android has stopped the process. Offline acceptance still works.
9. Include user-started pending discovery in the existing foreground-service and
   notification accounting, with Waiting for peers as its status. Do not require
   file URIs or nonzero downloaded bytes to recognize this work. Respect service
   suppression, manual stop and platform restrictions; avoid a timer/wakelock that
   spins merely because an offline or seedless torrent is saved.

**Acceptance:** add offline/zero peers; close the sheet; relaunch; provide a local
metadata-capable seed later; prove the same entry transitions to downloading
exactly once with the expected selected files. Repeat with manual pause/delete
before metadata, engine initially unavailable, duplicate magnet in another encoding,
destination failure and process death between metadata and descriptor attachment.
Check unknown size displays as unknown, never Complete; private metadata receives
no supplemental public trackers. No user re-add is required for ordinary recovery.

### F10 — Stop or resume individual files from download details

**User-visible contract:** each file in `TorrentFilesScreen` has a Download
checkbox. Unchecking stops that file's download demand and keeps saved bytes;
checking it again resumes that file. Other selected files continue. Display file
state (Downloading, Stopped, Checking or Complete), saved bytes and update errors.

**Implementation:** `TorrentFilesScreen.kt`, `MainActivity.kt`, `LibrarySession.kt`,
`DownloadStorage.kt`, `EngineClient.kt`; `engine-go/server.go`, `storage.go`,
`verification.go`, `playback.go`, DTOs and integration tests.

**Current blockers:** checkboxes modify only the preparation draft today. Native
`/select` adjusts priorities but does not attach user-visible storage for a file
that was initially unselected. `AttachDescriptors` is one-shot, and unselected
files use private boundary storage. Do not call initial configuration again or
merely flip UI selection and assume storage/piece state follows.

1. Route changes by stable entry key and file index through per-entry command
   serialization. Show Updating while the command applies; coalesce rapid taps to
   latest intent. Persist requested/applied selection consistently so failure or
   crash cannot make UI and native priorities silently disagree. Preserve an
   explicit torrent-level pause: rechecking a file does not override that pause.
2. On uncheck, clear that file's normal and focus/playback demands; recompute the
   union of remaining selected files. Retain its media and saved descriptor where
   safe. Cancel active incomplete playback demands for the stopped file so a
   player cannot silently keep it downloading. Explain that stopping that file
   can interrupt its stream; do not pause other files or erase media.
3. Already requested chunks may finish, and a selected neighbor may need bytes
   from a shared boundary piece. Account for that explicitly: “Stopped” means no
   independent demand for this file, not a guarantee that zero more bytes of a
   shared torrent piece can arrive. Preserve boundary bytes and verification.
4. Reselecting a previously attached file reuses its saved data. Selecting an
   initially unselected file requires a safe storage-extension operation: validate
   indices and expected generation, open/duplicate new destination descriptors,
   preserve useful boundary bytes, and commit the new mapping/selection together.
   Keep old mappings live until new handles are valid; roll back only newly created
   resources if opening/copying/configuration fails. Never truncate existing user
   media to force attachment. Native verification must use the final storage map.
5. Verify affected saved/newly promoted piece ranges before claiming them complete
   or permitting unverified reads. Coordinate with any running restore check:
   cancel/join or safely extend it without racing descriptor replacement. Prefer
   keeping unrelated files active; if a short torrent-wide storage/check transition
   is necessary for correctness, show Checking files and bound its scope rather
   than silently restarting/full-rechecking the entire download on every toggle.
6. Permit all files to be unchecked. Persist an explicit No files selected state,
   retain metadata/files and disable payload. It
   is neither a completed download nor a missing-metadata entry. Update all
   completion, ETA, notification, parse/load and startup rules to handle the empty
   set. Checking a file later restarts eligible work under current user policy.
7. Recompute selected total/progress from the current selection; file-level saved
   progress remains available for stopped files. Deselecting unfinished files may
   make the remaining selection complete: label Completed selected files rather
   than implying all torrent files are present. F7 should not notify completion
   for an empty selection. Invalidate only relevant preview/focus state.
8. A completed saved file remains locally playable when unchecked, if accessible;
   separate local-file playback permission from permission to fetch torrent data.
   Do not route an incomplete deselected file through the streaming engine without
   explicitly reselecting it. Audit `PlaybackProvider.target`, which currently
   requires selection for both local and incomplete playback.

**Acceptance:** two-file torrent: uncheck A while B keeps downloading, restart,
then recheck A and retain verified bytes. Also cover an initially unselected C,
all unchecked/relaunch/recheck, file boundary overlap, rapid repeated toggles,
storage-open failure, no space, pause during toggle, delete during promotion,
toggle during saved-data checking, completed deselected local playback, and an
active stream on A that cannot restore A's download demand after uncheck.

## 12. APK milestones, delivery boundaries and feature checklist

| Milestone | Scope | Why this order |
| --- | --- | --- |
| A — Stability | Required phases 0–6 | Restore/playback correctness before new policy/state. |
| B — Requested download workflow | F2, F9, F10 | Prioritize adding without peers and stopping individual files. |
| C — Everyday use | F1, F8, F7 | Easier control, useful notifications and better test reports. |
| D — Transfer control | F4 | Network restrictions respect manual intent; F3 removed. |
| E — Storage and playback | F5, F6 | Larger storage work and player-facing changes get their own test APK. |

Milestones are development increments, not automatic GitHub releases. For each,
update the report, run relevant regressions plus Phase 6 validation, produce a
distinct APK/checksum and hand it to the user. Build with compatible signing and
preserve library migration. If phone feedback exposes a regression, fix it before
expanding that area. Do not hold back milestone A until every feature is finished.

- [ ] A: stability APK and evidence delivered.
- [ ] Shared user-intent/automatic-reason decisions tested before F2/F4/F9/F10.
- [ ] F2: actionable wait/error states and bounded retry.
- [ ] F9: peerless/offline add persists and activates when metadata arrives.
- [ ] F10: details-file stop/reselect preserves bytes and other downloads.
- [ ] B: requested-workflow APK and restart/storage-boundary evidence delivered.
- [ ] F1: filters, stable sorting and partial-success batch controls.
- [ ] F8: safe, useful diagnostic export.
- [ ] F7: deduplicated completion notifications and safe actions.
- [ ] C: everyday-use APK and evidence delivered.
- [x] F3: withdrawn by user; slot queue/controls removed; legacy prefs cleared; eligible downloads run together.
- [ ] F4: network transfer policy and measured global rate caps.
- [ ] D: transfer-control APK and migration/restart evidence delivered.
- [ ] F5: destination selection and non-destructive storage repair.
- [ ] F6: file focus and truthful verified-byte availability.
- [ ] E: storage/playback APK and device evidence delivered.

For each feature, report what is implemented, its default, exact tests and any
remaining device/provider limitations. Leave performance numbers blank until
measured. A UI control without its persistence, cancellation and error path is
not a completed feature.

Persistent verified-piece checkpoints and playing during restore are deferred:
both need a separate design for changed files, interrupted writes and power loss.
Do not implement “trust saved progress,” aggressive tracker probing, hundreds of
extra trackers, automatic peer-limit increases, a new media player or additional
runtime dependencies as shortcuts in this plan.

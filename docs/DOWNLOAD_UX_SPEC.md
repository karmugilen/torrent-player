# Download controls and video experience specification

Date: 2026-09-13 · Status: proposed implementation plan only

## Scope and interpretation

Deliver two groups of improvements: reliable download controls, then a simpler add/video experience. This document specifies behavior and acceptance checks; it does not change the app.

- Notification Pause must actually stop transfer activity. Provide a clearly visible Stop action.
- Swiping away the download notification should stop transfers, preserve files, and keep the notification dismissed. This interprets “swiped” as the notification, not deleting a library row or closing the app from Recents.
- Treat the brief background/deletion `HTTP 404` as a bug to fix, not a request to display an error for one second.
- Add **Stop all & exit**, real video-frame thumbnails, one page for adding and selecting files, and download details with a live piece map.
- Keep the existing external-player workflow. An embedded player is optional future work.

## Current implementation findings

| Area | Observed code | Implication |
| --- | --- | --- |
| Notification | `PlayService.kt` already creates Pause/Resume and Stop actions, with `setOngoing(true)` and no dismissal handler. | Investigate action visibility and lifecycle; do not simply add a duplicate Stop button. |
| Pause | The Go engine exposes acknowledged pause/resume operations. | Verify real transfer shutdown and reflect acknowledged results. |
| Stop | The service stops immediately while session cleanup runs asynchronously. `syncService()` can still see active entries. | A service restart race is plausible; reproduce before claiming a confirmed cause. |
| Delete / 404 | Removal happens before some UI state changes; the poll loop keeps snapshots of engine IDs. | An in-flight poll can target an intentionally removed torrent. This is a likely, unconfirmed cause of the flash. |
| Add | `AddSheet.kt` collects input; `addCurrent()` navigates to `Screen.Prepare`. | Merge input, metadata progress, selection, and Download into one surface. |
| Video details | `TorrentFilesScreen.kt` shows selected files and progress. A single selected file opens the player directly. | Add an explicit Details action and enrich the existing files screen. |
| Telemetry | The Go engine exposes speed, peers, progress, ETA, and piece buckets. | Keep visualization based on real engine state. |
| Runtime | `EngineHost.kt` starts the native Go library once. | Keep lifecycle and recovery explicit. |

## 1. Notification, pause, stop, and exit

### Control semantics

| Action | Required result | Files and restart behavior |
| --- | --- | --- |
| Pause / Pause all | Stop new requests and disconnect transfer peers; acknowledge engine success before showing Paused. | Keep files and selection. Resume continues existing progress. |
| Resume / Resume all | Resume eligible paused downloads; report individual failures accurately. | Do not restart completed or deleting entries. |
| Stop / Stop all | Detach torrents, close associated streams and transfers, and remove the transfer notification once shutdown is acknowledged. | Keep files and entries in Stopped state. Restart only through explicit Resume. |
| Swipe notification | Route through the same Stop all operation wherever dismissal is supported. | Never erase files or automatically repost from polling. |
| Stop all & exit | Cancel draft preparation, stop all torrent network activity including seeding, persist state, stop service, and close the app activity. | No automatic resume on next launch. Completed local videos remain usable. |
| Delete | Retain explicit Keep files versus Delete files choices. | Only Delete files erases user downloads. |

### Notification presentation

- Active: title, selected-byte progress, aggregate download speed, Pause all and Stop all. Use singular labels for one transfer.
- Paused: accurate Paused text, Resume all and Stop all; no indeterminate download animation.
- Completed or stopped: remove transfer notification. An optional completion notification must be separately dismissible.
- Action taps immediately show pending state; resolve from engine acknowledgments. Repeated taps are idempotent.
- In a partially failed Pause all, show the remaining active count and a retry action. Do not claim all transfers stopped.
- Make Stop all available in the app as well, so notification layout is never the only access path.
- Verify foreground-service dismissal behavior on supported Android versions and target devices during implementation. Where active notification swipe cannot be supported, keep Stop all visible and make the paused notification dismissible. Do not hide ongoing transfers behind a dismissed notification.

### Lifecycle design

- Use explicit entry states: Preparing, Downloading, Pausing, Paused, Stopping, Stopped, Completed, Deleting, Error. Keep a recoverable previous state for failed commands.
- Put commands through one session coordinator shared by notification and UI actions. Serialize conflicting operations per entry and use a global shutdown gate for Stop all & exit.
- Prevent new add, resume, restore, thumbnail network work, and notification reposts while shutting down.
- Remove stopping/deleting IDs from polling eligibility immediately; invalidate in-flight results using an operation generation, without losing the metadata required for recovery.
- Stop the service after transfer cleanup, not before launching asynchronous cleanup. A timeout must produce a recoverable failure, never a false successful shutdown.
- Persist stopped/paused intent before allowing restore. Surface persistence failure instead of silently risking an unwanted resume.
- Initial exit contract: no torrent traffic, active streams, or foreground notification; the OS may retain the process.
- Explain that stopping an active streamed video ends its stream. Local-file playback in another app is independent.

### Acceptance checks

- Pause from the notification while the app is backgrounded: transfer peers disconnect and received bytes stop increasing after bounded in-flight work drains. Define and measure that bound on device; target two seconds on the local test fixture.
- Resume retains verified progress and file selection. A failed pause never shows a successful Paused state.
- Stop or supported swipe: no notification reappears during 30 seconds of observation, no new torrent traffic occurs, and relaunch does not auto-resume.
- Test one download, multiple downloads with mixed states, preparation in progress, completion, seeding, and active streaming.
- Stop all & exit preserves files and supports later explicit Resume without duplicate entries.

## 2. Fast deletion and no transient HTTP 404

### Required behavior

- Immediately close the confirmed delete dialog and render Removing…; target under 100 ms of UI response. Actual storage cleanup remains asynchronous.
- Keep the row noninteractive while cleanup runs. Remove it on success; on failure retain a recoverable row with a useful error and retry. Do not promise instant physical deletion.
- Exclude deleting entries from notification totals and normal polling. Discard late results from a superseded operation.
- An engine “torrent not found” response for an intentionally removed ID counts as already removed. Continue requested file cleanup.
- Suppress only expected lifecycle responses. A missing active torrent, missing video URI, remote torrent URL 404, or failed storage deletion remains actionable.
- Scope errors to their operation and entry. Old errors must not flash on the library, a newly opened add page, or an unrelated torrent.
- Keep status code, endpoint, entry ID, and operation generation in diagnostic logs; show readable user messages instead of raw `HTTP 404`.
- Propagate coroutine cancellation instead of converting it into a failure message or silently continuing cleanup.

### Acceptance checks

Use deterministic delayed responses to exercise poll-versus-delete, poll-versus-stop, duplicate removal, cancellation during metadata loading, and reopening the add page. No expected removal produces an error flash. Genuine failures remain visible. Verify Keep files preserves content and Delete files removes only the requested files; partial storage failures retain enough information to retry.

## 3. One page: add → choose files → download

Use one full-height Add download surface, reusing existing selection logic rather than navigating from a bottom sheet to another page.

```text
Add download                                      Close
[ Magnet or torrent URL                              ]
[ Open torrent file ]

Finding metadata… / torrent title and file count
[ All ] [ Videos ] [ None ]      [ Search files… ]
[✓] Movie.mkv                                  1.4 GB
[ ] Extras/interview.mp4                        180 MB
[✓] Movie.srt                                   52 KB

2 files selected · 1.4 GB · destination / free space
[ Download selected                                  ]
```

- Valid pasted input starts debounced metadata lookup inline; file import enters the same screen.
- Show metadata-loading, no-peer, invalid-input, retry, and ready states in place. A magnet cannot show its file list until metadata arrives.
- MVP preparation fetches metadata only; payload downloads begin after Download selected. Adjust the current prefetch behavior accordingly.
- Keep input and selection visible while loading. Editing the source cancels the previous draft; generation checks prevent older responses replacing the new result.
- Select video files by default, consistent with current behavior; make the choice visible. Preserve explicit user changes across refreshes. For non-video torrents, retain a clear all-files default.
- Show filename/path, size, selected count, selected total bytes, destination, and available space where measurable. Support scrolling and large file lists.
- Disable Download until metadata is ready, selection is nonempty, and storage is usable. Show the reason beside the button.
- Download is the single commit action: allocate selected files, configure once, add one library entry, then return to Library.
- Disable duplicate commits and detect existing info hashes. Offer Open existing instead of a duplicate download.
- Close cancels the draft and cleans temporary preparation resources without touching existing downloads.

Acceptance: paste → choose files → Download occurs on one surface; `.torrent` import follows the same flow; slow metadata, source changes, cancellation, double taps, duplicates, and storage failure remain recoverable. Unselected payload is not deliberately requested; shared torrent-piece boundaries may include adjacent bytes.

## 4. Real video thumbnails

- Extract a frame from the actual downloaded video, using its accessible content URI/file descriptor. Do not substitute unrelated poster art.
- First candidate: 35% of video duration. If too dark or invalid, try 20% and 50%, with at most three attempts. For unknown duration, attempt the first decodable frame.
- MVP generates thumbnails after file completion. Partial-file previews are a later enhancement: unavailable container metadata or frame bytes must never stall the UI or start hidden downloads.
- Proposed decoder: Android `MediaMetadataRetriever`; validate supported formats and provider seek behavior on fixtures before committing to the implementation.
- Run extraction off the UI thread with one worker, bounded image size, cancellation, and a time budget. Release decoder and descriptors on all paths. Validate that the chosen timeout strategy actually bounds native decoder work.
- Cache by torrent info hash, file index, file identity/version, and thumbnail algorithm version; use a bounded cache, initially 50 MB, with least-recently-used eviction.
- Render a stable 16:9 preview above the filename in library cards and beside/above each video in Details. Keep progress and controls below the title; no floating image between unrelated controls.
- For multi-video torrents, use the largest selected video for the library cover, with individual thumbnails in Details. Use a neutral video placeholder until ready.
- Regenerate after file replacement; invalidate on removal. Decode failure is a placeholder, not a download error. Provide an accessible video name and duration when known.

Acceptance: completed MP4, MKV, short clip, black opening scene, unsupported codec, revoked URI access, and corrupt video all leave scrolling responsive. Cached thumbnails survive reopening and never show another file’s image.

## 5. Download details and live piece map

### Navigation and layout

- Keep single-video card tap as Play; provide a clearly labeled Details action on every library entry.
- Multi-file torrent tap opens Details, evolving the existing Files screen. Each video retains its own Play action.
- Order: cover/title → status and controls → speed/bytes/ETA/peers → live map → selected files → expandable technical information.
- Metrics: verified selected bytes / selected total, download and upload speed, connected peers, ETA, and per-file progress. Show “Estimating…” when ETA is unknown.
- Technical information: info hash, destination, selected versus total file count, piece size/count, and available tracker diagnostics. Never invent seed counts or availability.

### Visualization contract

- Label the visualization **Pieces**. Each tile represents a contiguous range of torrent pieces; it is not an exact network-block viewer.
- Legend: missing, receiving, verified, excluded. Use color plus patterns/labels for accessibility. If a group contains mixed states, show proportions or a mixed state rather than marking it fully verified.
- Map full torrent order left to right with file boundaries and a selected-files filter. Account for files sharing boundary pieces; never imply byte-exact file isolation.
- Add an on-demand piece telemetry endpoint, provisionally `GET /pieces/:id?maxBuckets=256`.
- Proposed response: torrent/session generation, sample timestamp, total piece count, piece length, and up to 256 buckets with index range, selected-piece count, verified-piece count, and receiving-piece count. Define overlap rules explicitly; receiving excludes verified, while selection is a separate dimension.
- Build state from real verified-piece data and actual outstanding requests. If receiving state cannot be measured reliably, omit it and adjust the legend.
- Fetch at most once per second only while Details is visible and the app is foregrounded. Pause freezes activity; completion leaves a stable verified map. Missing telemetry shows “Map unavailable” without interrupting playback/download.
- Bound response size and computation; cache/group engine data rather than sending raw bitfields for every library poll.

Acceptance: a controlled torrent with known piece completion matches the displayed buckets; out-of-order completion is visible; paused torrents show no continuing receiving animation; deselected files and shared boundaries are correct; large torrents remain responsive. Completed local playback works without an engine session.

## Review-driven implementation clarifications

The following decisions make the requirements implementable against the current Android and Go-engine architecture.

### Shared lifecycle and race rules

- Add a per-entry operation generation and an `isDeleting`/explicit lifecycle state to the persisted model. Increment the generation before Pause, Resume, Stop, Delete, source replacement, or exit work; update UI state synchronously so polling and notification aggregation exclude the entry immediately.
- A poll captures the entry key, engine ID, and generation. Apply its result only if all three still match when the response returns. Treat a 404 as silent success only for an ID invalidated by the current remove/stop/delete operation; surface other missing-session and storage failures as recoverable errors.
- Route notification and UI commands through the same serialized session coordinator. A command is acknowledged only after the engine result and state persistence succeed. Failed commands restore the prior state and identify affected entries.
- Cleanup of an uncommitted metadata-preparation torrent is mandatory on Close, source replacement, cancellation, timeout, and every failure path, including a torrent ID returned after cancellation. Cleanup that must preserve files runs in `NonCancellable` context.

### Notification and service contract

- Attach a `deleteIntent` to the transfer notification wherever Android permits dismissal callbacks. Dismissal invokes the same idempotent Stop all operation and sets a persisted service-suppressed gate before asynchronous cleanup begins.
- Do not call `stopForeground` or `stopSelf` until the session coordinator reports that engine cleanup and state persistence have completed. Polling must observe the suppression gate and cannot repost the notification during shutdown.
- The paused presentation must define whether the foreground service is stopped while a normal dismissible notification remains, according to the target Android version and device behavior. Active transfers retain an always-visible Stop all action when dismissal cannot be intercepted.
- Stop all & exit ends transfer activity, persists stopped state, stops the service, and finishes the Activity without killing the Android process.
- Expose Stop all & exit in an app-level action, such as the Library overflow menu and Settings App actions, and state that it cancels metadata preparation and telemetry jobs as well as downloads and seeding.

### Deletion and storage contract

- On confirmation, clear the delete dialog and mark the row `Removing…` in the first UI state mutation, before engine or SAF I/O. The row is noninteractive and is excluded from polling and notification totals.
- Engine removal is idempotent for an intentionally removed ID. A 404 must not prevent Keep files or Delete files cleanup. Physical storage failure retains a retryable row and never reports success.
- Free-space checks use the actual destination volume: the default Downloads volume or the selected SAF tree, with an explicit “space unavailable” state when it cannot be measured. The destination and the reason for a disabled Download action remain visible beside the action.
- Delete cleanup invalidates thumbnails for the affected info hash. Expected lifecycle errors are suppressed only when their operation generation and target match; remote source 404s and genuine storage failures remain actionable.

### Unified add flow contract

- The Add surface remains full-height while input, destination, loading/error state, file filters, and selection are visible. Metadata lookup must not request payload pieces or write payload bytes before the final Download selected action.
- Source edits, imports, Close, and cancellation increment the draft generation. Late metadata, status, or error responses are ignored unless they belong to the current generation.
- Duplicate detection occurs after metadata resolves for magnets, URLs, and imported `.torrent` files by info hash. Disable the commit and offer Open existing, which navigates to the existing entry without replacing it. The final commit is guarded synchronously against double taps.
- The final commit allocates storage on the chosen destination, configures the engine once, persists one library entry, and rolls back temporary files and engine state on failure. A partially completed configure is retryable and cannot strand a preparation session.
- The default selection is all video files, or all files when no video exists. Preserve explicit selection changes across metadata refreshes, show Videos and Search filters, and show selected bytes, destination, and measured free space.

### Thumbnail and Details feasibility rules

- The thumbnail repository scans completed entries after cold start and queues visible items ahead of background work. It uses a disk cache capped at 50 MB plus a small memory LRU, keyed by info hash, file identity/version, and algorithm version; failed or unsupported extraction receives a negative cache marker.
- On API 27 and later, use bounded scaled extraction. On API 26, use a safe downsample path and catch decoder memory failures. Native extraction runs on a dedicated worker with a watchdog and replacement path for a wedged decoder; coroutine cancellation alone is not considered a native-call timeout.
- Define “too dark” as average BT.601 luminance below 18 over a 16×9 sample grid. Try 35%, 20%, and 50% in that order, then use a neutral placeholder. Thumbnail failure never becomes a download error.
- Rename or evolve `Screen.Files` as Details. Every library entry has an explicit Details action; a single-video tap may still play immediately. Non-video files use their detected MIME type and an Open action rather than a video intent. Removing the open entry returns the user to Library.
- Compute ETA from verified selected bytes and current download speed, not whole-torrent time remaining. Show “Estimating…” when speed or selected progress is insufficient. Details displays upload speed, peers, selected-file progress, and truthful tracker diagnostics.
- Piece buckets use deterministic contiguous ranges: if total pieces is at most `maxBuckets`, one bucket per piece; otherwise partition the full range into at most `maxBuckets` nonempty ranges using `ceil(totalPieces / maxBuckets)`. Each bucket reports its range, selected, verified, and receiving counts; receiving excludes verified. A boundary piece is selected if any selected file overlaps it.
- Fetch piece telemetry only while Details is foreground-visible, at most once per second. Render the map in one Compose Canvas, freeze receiving state on pause, and show “Map unavailable” when the engine cannot provide telemetry. Completed local playback does not depend on an engine session.

### Additional acceptance checks

- Cancel or replace metadata preparation after the engine has returned an ID: no orphan torrent remains, no stale file list appears, and no payload bytes transfer before commit.
- Importing a duplicate `.torrent` or URL offers Open existing and never overwrites the existing library entry; two rapid commit taps create at most one entry.
- Confirm deletion closes the dialog within 100 ms, shows Removing…, and a delayed poll or engine 404 produces no transient error. Keep files preserves content; Delete files removes only the requested content.
- On Android API 26, a 4K video thumbnail remains responsive without `NoSuchMethodError` or unbounded bitmap allocation. Corrupt, unsupported, revoked, and black-opening videos fall back to a cached placeholder and do not retry on every recomposition.
- A single-file entry exposes Details, selected-file ETA reflects only selected bytes, a 12-piece torrent returns 12 one-piece buckets, and a 50,000-piece torrent remains responsive while its map updates.

## Implementation order and ownership

| Phase | Main files | Deliverable |
| --- | --- | --- |
| 1 — reliable controls | `PlayService.kt`, `LibrarySession.kt`, `UiModels.kt`, `engine-go/server.go`, `EngineClient.kt` | Acknowledged pause/stop, shutdown gate, stale-response protection, deletion fixes, Stop all & exit. |
| 2 — unified add | `AddSheet.kt`, `PrepareScreen.kt`, `MainActivity.kt`, `MainViewModel.kt`, `LibrarySession.kt` | One add-and-select surface, single commit, cancellable metadata-only preparation. |
| 3 — thumbnails and details | `LibraryScreen.kt`, `TorrentFilesScreen.kt`, `DownloadStorage.kt`, new thumbnail repository | Cached frame previews, explicit Details, richer file information. |
| 4 — truthful live map | `engine-go/server.go`, `EngineClient.kt`, `TorrentFilesScreen.kt` | Bounded on-demand piece telemetry and accessible map. |

Verify each phase before proceeding: meaningful coordinator/race tests, engine protocol tests, then Android device checks for notification behavior, background transfers, storage providers, external playback, and thumbnail decoding. No runtime tests are claimed by this planning document.

## Optional future work, after these fixes

| Priority | Feature | Completion condition |
| --- | --- | --- |
| Next | Queue and concurrency limit | Configurable active-download count; queued items consume no payload bandwidth. |
| Next | Wi-Fi-only and bandwidth limits | Consistent enforcement across foreground, background, and resumed downloads. |
| Next | Storage preflight and recovery | Insufficient space and revoked destination access have direct recovery actions. |
| Later | Partial-video thumbnails and readiness indicator | Bounded extraction using available bytes; no misleading “playable” guarantee from percentage alone. |
| Later | Tracker health and retry diagnostics | Useful per-tracker status without flooding users with transient errors. |
| Later | Embedded player and resume position | Seek, subtitle, lifecycle, and partial-file buffering behavior validated end to end. |

These are separate follow-up features, not prerequisites for the requested notification and add-flow improvements.

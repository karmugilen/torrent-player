# Local redesign tickets

Agreement: [REDESIGN_SPEC.md](REDESIGN_SPEC.md). Baseline: `a226754`.
All delivery is local; no GitHub push or remote tickets.

**Implementation gate: wait for the user to explicitly say to start.**
No ticket below has been implemented. Documentation and local planning commits
are the only changes so far. "Planned" indicates dependency order, not permission
to begin coding, building an APK, installing, or deploying.

| Ticket | State | Dependencies | Deliverable |
| --- | --- | --- | --- |
| UI-01 | Planned | — | Shared Aero, Antiquity and Retro design system; saved theme/effects preferences. |
| UI-02 | Planned | UI-01 | Watch-first library, Continue watching, filters/sort, safe batch controls and useful empty states. |
| UI-03 | Planned | UI-01 | Friendlier Add, episode/file browsing, and playback-first details with themed controls. |
| UI-04 | Planned | UI-01 | Replace circular reactor with expandable branching peer view; keep world map. |
| MEDIA-01 | Planned | — | Automatic persistent thumbnails, decoder fallbacks and truthful preview states. |
| MEDIA-02 | Planned | MEDIA-01 | Bounded verified preview reads for Direct Play/incomplete media without external-player side effects. |
| NET-01 | Planned; user trial | — | Built-in experimental anti-DPI On/Off switch in Settings; off by default; user tests effectiveness. |
| REL-01 | Planned; reliability priority | — | Handle checkpoint sync/write failures without losing pending recovery state. |
| CACHE-01 | Planned | — | Per-torrent metadata and bounded last-known-good library recovery snapshots. |
| CACHE-02 | Planned | — | Persistent, expiring hints for recently successful public-torrent peers. |
| CACHE-03 | Planned | — | Warm DHT bootstrap using bounded, revalidated node contacts. |
| CACHE-04 | Planned | REL-01, CACHE-07 | Small verified persistent resume window within explicit disk limits. |
| CACHE-05 | Planned | MEDIA-01 | Reusable video information and successful thumbnail timestamps. |
| CACHE-06 | Planned | UI-01, UI-02 | Saved navigation state and cached static theme decoration. |
| CACHE-07 | Planned | MEDIA-01 | Consistent cache accounting, limits, cleanup and active-reader protection. |
| QA-01 | Planned | UI-01–04, MEDIA-01–02 | Focused regressions, existing suites, local APK and visual verification where available. |
| STORAGE-01 | Backlog | First redesign | Non-destructive moved-file/folder-access repair and clearer storage recovery. |

## Rules shared by all tickets

- External Android players are mandatory. Preserve URI grants, playlists,
  subtitle sidecars and supported player-result resume contracts.
- Do not implement the rejected adaptive/evolutionary architecture proposal,
  retune peer limits/buffering, or add a competing scheduler.
- Preserve saved libraries, download selection, explicit pause/stop/delete, and
  verified data. Additive persisted settings must tolerate old/missing fields.
- Update this local board with actual results and local commit IDs when work is
  later authorized. Do not push, publish, or create remote issues.
- Cache additions must have a stable identity, owner, size limit, invalidation
  rule and safe miss/corruption path. Keep library records/bookmarks in durable
  storage; regenerable cache eviction must not remove user intent or media.
- Do not use cached peer counts, rates, folder permission, file existence or
  downloaded percentages as current truth. Preserve verification of media bytes.

## Cache improvement coverage

The user requested all eight areas discussed in the cache review, plus the
checkpoint reliability finding. Thumbnail persistence already belongs to
MEDIA-01 and is extended there rather than duplicated as another implementation.

| Requested area | Ticket |
| --- | --- |
| Persistent thumbnails for incomplete downloads and Direct Play | MEDIA-01 |
| Library/metadata recovery | CACHE-01 |
| Recent successful public peers | CACHE-02 |
| DHT bootstrap contacts | CACHE-03 |
| Verified video startup/resume reuse | CACHE-04 |
| Video information and successful preview locations | CACHE-05 |
| UI state and theme-rendering reuse | CACHE-06 |
| Cache sizes, limits and complete cleanup controls | CACHE-07 |
| Checkpoint durability error handling | REL-01 |

Existing metadata persistence, cached tracker lists, verified startup pieces and
RAM playback caches are starting points to preserve. Do not create duplicate
cache layers or claim these existing mechanisms as new features.

## UI-01 — Three complete themes

**Likely files:** `Theme.kt`, `UiModels.kt`, `LibrarySession.kt`,
`SettingsScreen.kt`, `MainActivity.kt`, reusable UI assets/controls as needed.

**Work:**

- Define shared visual values and reusable panels, controls and screen chrome.
  Create distinct Aero, Antiquity and Retro treatments, not independent copies
  of each screen's behavior.
- Save theme and reduced-effects preferences. Use Aero for the new theme default
  and preserve the existing appearance preference. Handle unknown saved themes.
- Apply the treatment across Library, Add, Prepare, Details, Settings and dialogs.
- Keep procedural/static decoration cached; avoid constant full-screen redraws.

**Acceptance:**

- Three visually distinct selectable themes with Aero as default; selections
  survive restart. Shared functionality across themes.
- Glass/gloss, parchment/botanical/celestial ornament and classic bevels are real
  treatments, not only palette changes. Text remains readable.
- Reduced effects persists and suppresses nonessential motion.
- Wallpaper-derived colors do not replace the selected theme's identity.
- Check small screens, larger text, touch targets and light/dark appearances.

## UI-02 — Watch-first library and everyday controls

**Likely files:** `LibraryScreen.kt`, `UiModels.kt`, `LibrarySession.kt`,
`DownloadStorage.kt` only if additional bookmark metadata is necessary.

**Work:**

- Add a Continue watching area that targets the correct bookmarked episode.
- Expose the existing stable sort choices and meaningful status filters.
- Add stable-key selection and batch pause/resume/remove, reporting partial
  failures without undoing successful actions or operating on stale entries.
- Provide separate first-use, no-search-results and no-filter-results messages.

**Acceptance:**

- Library exposes All/Active/Ready/Needs attention, stable sorting and search.
- Continue watching opens the bookmarked episode in an external player.
- Multi-selection uses stable keys and handles entries disappearing or failing
  individually. Destructive actions retain explicit keep-files/delete-files choice.
- A search/filter with no matches does not claim the library is empty.
- Direct Play is not classified as a completed offline download. Filters handle
  pending metadata, checking, empty selection, paused and deleting entries.
- Continue watching is shown only when a real supported player bookmark exists;
  no invented watch progress for players that return no position.

**Verification:** focused tests for filters, sort ties, resume target and batch
selection/error behavior; visual checks for long names and a populated library.

## UI-03 — Friendly Add and playback-first details

**Likely files:** `AddSheet.kt`, `PrepareScreen.kt`, `TorrentFilesScreen.kt`,
`PlaybackLaunch.kt`, `MainActivity.kt`; existing selection helpers as needed.

**Work:**

- Use clear Watch/Save wording while explaining Direct Play versus offline files.
- Put files and episode playback ahead of transfer/map/tracker/piece diagnostics.
- Add file search and natural episode ordering, with clear selected/stopped states.
- Make subtitle selection understandable and retain compatible external-player
  playlist, subtitle and resume extras.

**Acceptance:**

- Playable files appear before connection/tracker/piece diagnostics.
- Search, natural episode order and video/subtitle selection work with existing
  file-selection persistence. No embedded video player.
- Add, Direct Play, pause/resume and deletion retain existing intent and error paths.
- Adding without peers still saves the pending entry. Duplicate magnets open the
  existing item. Filtering files does not silently change selection.

**Verification:** existing file-selection/external-playback tests plus focused
coverage for any new ordering/search/selection rules; manually inspect both add modes.

## UI-04 — Branching connection visualization

**Likely files:** `ConnectionReactor.kt` (replace/rename), `ConnectionMap.kt`,
`TorrentFilesScreen.kt`. Preserve `PeerWorldMap.kt` behavior.

**Work:**

- Replace the circular reactor with a portrait-friendly device-at-top diagram,
  branching into TCP/uTP/WebRTC groups and rectangular peer tiles.
- Keep deterministic ordering and expandable summaries for large peer groups.
- Show available country/client/protocol information and actual transfer activity.
  Compute displayed rates from counter deltas and elapsed time if rates are added.
- Retain lifecycle-bound refresh. Animate links only when payload changes and
  reduced effects is off. Provide an accessible textual equivalent.

**Acceptance:**

- Geographic map remains available. No circular reactor in the alternative view.
- Device branches into protocol groups and rectangular peer tiles; stable order,
  expandable groups, real activity, useful empty/paused/unavailable states.
- No fabricated latency, seeder status, throughput or country data.
- Idle connected peers are distinguished from transferring peers. Reset counters,
  unknown locations and unavailable snapshots do not create fake activity.

**Verification:** inspect zero/one/many-peer states, mixed protocols, pause,
screen leave/return, reduced effects, long client labels and large text.

## MEDIA-01 — Required automatic thumbnail lifecycle

**Likely files:** `ThumbnailRepository.kt`, `LibraryScreen.kt`,
`TorrentFilesScreen.kt`, `Formatters.kt`; pure preview-state helpers as needed.

**Work:**

- Attempt thumbnails automatically on both Wi-Fi and mobile. Do not add a
  Wi-Fi-only gate or a manual Load preview requirement.
- Load/persist a successful first image for incomplete and Direct Play entries.
  Use stable torrent/file identity so a restart or URI change does not lose it.
- Try decoder fallbacks independently; prefer non-dark frames and accept a dark
  real frame as the last successful fallback. Remove permanent failure poisoning.
- Align video recognition with supported MIME mappings, while acknowledging
  that actual decoding depends on the device's codecs.
- Add clear loading/waiting/unavailable states and bounded automatic retry.
  Deduplicate extraction across library/details and avoid needless re-extraction.
- Load cached images before requiring an engine session, complete download or
  current URI. Keep cache identity independent of a movable provider URI.
- Write image replacements atomically and retain an existing good image when
  another extraction fails. Apply per-entry eviction on library removal and the
  reusable clearing/accounting rules in CACHE-07.

**Acceptance:**

- Automatic attempts on both Wi-Fi and mobile; no manual thumbnail button.
- Successful incomplete/Direct Play images survive process restart.
- Decode failures do not skip independent fallbacks or permanently poison cache.
- Cached thumbnail remains usable while paused/offline. Failed attempts are bounded.
- A failed or throwing decode attempt does not skip all later fallback methods.
- Unknown metadata, unavailable video bytes and unsupported/corrupt media have
  honest states; do not display generated artwork as a decoded video frame.
- Incomplete and Direct Play images survive ordinary process restart. Explicit
  cleanup or Android cache eviction causes a safe miss and automatic recovery,
  without losing metadata, selection, bookmarks or downloaded media.

**Verification:** test stable keys/cache invalidation/retry policy and fallback
selection. Device-check success on local media, incomplete data, Direct Play,
dark opening frames, process restart and decoding failure when implementation starts.

## MEDIA-02 — Preview without a full saved video

**Likely files:** `EngineHost.kt`, `PlaybackProvider.kt` or an app-private preview
data source, `ThumbnailRepository.kt`, JNI exports, `engine-go/playback.go` and
narrow storage helpers/tests. Determine the smallest safe change before coding.

**Work:**

- Supply seekable verified bytes to the extractor without requiring `SavedFile.uri`.
  Ordinary `OpenPlayback` cannot be used unchanged: it mutates playback focus and
  startup/resume hints and has playback-sized demands.
- Introduce preview-specific demand only where necessary, using the existing
  scheduler. Bound admitted pieces, concurrent work, elapsed wait and cleanup.
- Reuse verified cached bytes. Prefer one preview job at a time, with an initial
  engineering target of 8 MiB / 10 seconds per attempt to validate against piece
  sizes/container needs; budget exhaustion leads to fallback and bounded retry.
- Handle ready-but-unconfigured add drafts with temporary verified storage if
  previewing them; clean up or hand over safely on commit/cancel. Do not create a
  full-sized destination video merely to obtain a preview.
- Cancel fetching on pause/stop/delete and when the requester disappears. A
  restored paused Direct Play item can use its persistent image without reviving
  a transfer. Do not loosen the global payload gate to start full downloads.

**Acceptance:**

- Real verified torrent reads can supply a thumbnail without saving a full video.
- Preview demand is bounded and released on success, timeout, pause or deletion.
- Does not mutate playback focus/history/resume hints or change peer algorithms.
- A preview cannot fabricate verification, revive stopped transfers, or continue
  fetching indefinitely when decoding fails. Exercise piece overlap and cancellation.
- Do not mistake a coroutine timeout for proof that blocking native decoding was
  interrupted. Verify that reader shutdown releases demand and unblocks pending reads.
- A preview cannot consume another active Direct Play session's cache or change
  the selected episode. All networks means Wi-Fi/mobile availability, not reversing
  the user's explicit pause/stop or transfer restrictions.

**Verification:** deterministic local peer tests for verified reads, no full-file
creation, byte/piece budget, unavailable pieces, timeout, pause/delete, concurrent
playback, and cleanup before storage configuration/episode changes.

## NET-01 — Built-in experimental anti-DPI switch

**User request:** "no need to test just add in turn off on put in setting for me
so i can test and tell you." This settles delivery as a functional experimental
setting with user-run effectiveness trials; it does not require proving ISP
filtering before implementation. The standing implementation-start gate remains.

**Likely files:** `SettingsScreen.kt`, `UiModels.kt`, `LibrarySession.kt`,
`EngineClient.kt`, `engine-go/network.go`, client configuration/dialing helpers
and a narrowly scoped bundled backend/build integration if needed.

**Work:**

- Add **Network → Anti-DPI (experimental)** with an On/Off switch. Default Off;
  persist the choice and show whether the backend is actually available/active.
- Evaluate the smallest viable built-in Android path during implementation:
  an in-process stream adapter or app-managed local proxy is a candidate. Full
  packet-level zapret is not assumed to work without privileged packet access.
- Connect the switch to real network behavior. Do not deliver a UI-only toggle,
  a flag with no consumer, or existing MSE settings mislabeled as full anti-DPI.
- Specify coverage for outgoing TCP peers, HTTP/WebSocket trackers, UDP trackers,
  DHT, uTP, WebRTC and incoming connections. If the initial backend covers only
  some paths, state that plainly in the UI/help and local handoff.
- Define routing and fallback explicitly. The library races multiple dialers;
  merely appending a bypass dialer can let a direct connection win. Mark actual
  bypass use, unsupported paths and failures truthfully. Do not silently claim
  that untreated/direct connections were bypassed.
- Switching Off restores the normal route and closes bypass-owned resources.
  Surface any reconnect/restart requirement instead of implying existing sockets
  have changed route instantly. Preserve pause/stop/delete intent and loopback
  media/provider traffic used by mandatory external players.
- Do not change peer-selection algorithms, connection limits, Direct Play
  buffering or broad architecture as part of this ticket. A Rust rewrite is
  not required merely because a bypass backend is native.

**Acceptance:**

- The switch exists in Settings, starts Off on a fresh install, survives restart,
  and activates/deactivates a real supported implementation.
- Unsupported backend/startup errors are visible; the switch never claims an
  active bypass when activation failed.
- Exact supported transports, fallback behavior and any device requirements are
  documented. Avoid "hidden from ISP", "anonymous", or guaranteed extra peers.
- An installable local trial APK can be delivered when implementation is
  authorized; the user compares On/Off and reports results.
- **No upfront ISP testing, packet capture, public-network benchmarking or
  effectiveness proof is required.** Do not run those investigations. The
  user's feedback determines whether this experimental feature helps their ISP.
- No network effectiveness result is claimed from compilation or existing unit
  checks. Do not add a separate effectiveness-testing milestone before handoff.

**Decisions still technical/open:** exact backend, supported Android techniques,
transports and failed-bypass routing. Resolve these transparently during the
authorized implementation; do not assume full packet coverage or invisibility.

## REL-01 — Reliable verification-checkpoint persistence

**Priority:** address before adding durable resume bytes or extending checkpoint
reuse. This is a persistence fix, not adaptive buffering or a scheduler change.

**Likely files:** `engine-go/storage.go`, `checkpoint.go`, related recovery tests.

**Evidence:** code inspection of `drainSyncAndCheckpoint` found ignored errors
from file `Sync` and `writePieceCheckpoint`, followed by a path that clears
`checkpointDirty`. No disk-failure scenario has been reproduced in this planning
task; do not claim observed data loss.

**Work:**

- Preserve failed pending sync work and dirty checkpoint state until the relevant
  data and checkpoint are successfully persisted for the correct generation.
- Do not publish durable completion after a failed prerequisite file sync.
  Keep retries bounded/backed off, preserve files and surface useful errors.
- Coordinate writers with close, pause, selection changes and deletion. A stale
  worker must not recreate a deleted checkpoint or clear newer dirty state.
- Bound checkpoint reads/allocations before trusting counts from the file.
  Keep corruption fallback and existing piece/file identity validation.

**Acceptance:**

- A failed file sync or checkpoint write cannot be treated as a successful save.
- Later recovery can retry without discarding saved bytes or hiding the error.
- Malformed/truncated checkpoints are rejected safely, with verification fallback.
- No expanded trust in saved percentages or unverified data; no new peer policy.

**Verification when authorized:** fault-inject file-sync/checkpoint-write failures,
retry success, generation changes, close/delete races and malformed checkpoint
counts. Run focused existing resume regressions after the fix.

## CACHE-01 — Recoverable library snapshots and torrent metadata

**Likely files:** `DownloadStorage.kt`, `DownloadRestore.kt`, `LibrarySession.kt`,
small app-private persistence helpers if needed.

**Starting point:** metadata, selected files, destinations and bookmarks already
persist. This ticket improves recovery and avoids duplicating that information.

**Work:**

- Keep a bounded last-known-good library snapshot and versioned per-torrent
  metadata where it isolates recovery from a damaged mutable library record.
- Validate schema and torrent identity before making a new record authoritative.
  Save using atomic replacement and retain a known-good predecessor on failure.
- Load existing saved-library formats without losing pending magnets, explicit
  empty selection, paused Direct Play entries or external-player bookmarks.
- Define ownership, deletion/generation markers and orphan cleanup so recovering
  an old snapshot cannot silently undo a confirmed removal or resurrect transfers.

**Acceptance:**

- A corrupted record does not prevent restoring unrelated valid entries.
- Recovery identifies itself to the user and preserves damaged material for
  diagnosis rather than erasing it. No duplicate torrents or destination files.
- Missing metadata uses normal discovery; cache/snapshot failure never pretends
  metadata is valid. Removing a torrent also retires its owned recovery records.

**Verification when authorized:** legacy migration, truncated snapshot, partial
save, missing per-torrent metadata, duplicate identity, stale-generation recovery,
pending magnets and confirmed deletion followed by restart.

## CACHE-02 — Recent successful public-peer hints

**Likely files:** `engine-go/network.go`, `server.go`, a focused peer-hint store.

**Starting point:** route recovery retains up to 80 addresses in memory. It does
not currently provide persistent successful-peer history across process restart.

**Work:**

- Save a small per-torrent set of successfully connected endpoints, source,
  transport, last success and expiry. Use bounded records and a short TTL.
- Feed valid hints into existing discovery/dialing. Preserve connection limits,
  retry policy and scheduler ownership; do not create a new dialing loop.
- Revalidate after network changes and expire old failures. Use the currently
  selected route/bypass policy; cached IPs must not bypass NET-01 restrictions.
- Limit persistent torrent-peer hints to confirmed public torrents. Exclude
  private/unknown-privacy metadata and never mix hints between torrent identities.

**Acceptance:**

- Cached endpoints are candidates only, never displayed as connected peers.
- Stale/unreachable entries do not block fresh discovery or cause retry storms.
- Pause/stop/delete remain effective; deleting an entry retires its peer hints.
- Corrupt or absent cache files fall back to the current discovery behavior.

**Verification when authorized:** expiry/size bounds, restart, network generation,
duplicate endpoints, private/unknown torrent exclusion and canceled discovery.
Use deterministic local peers; no ISP-effectiveness benchmark is a prerequisite.

## CACHE-03 — Warm DHT bootstrap contacts

**Likely files:** `engine-go/server.go`, `network.go`, existing DHT startup hooks
and a bounded contact store; preserve the pinned library's routing decisions.

**Work:**

- Persist a bounded set of responsive DHT node endpoints as extra bootstrap
  candidates, separated by address family and aged by last successful response.
- Revalidate contacts and retain bundled/global bootstrap fallback. Do not
  restore stale announce tokens or assume old routing liveness after an IP change.
- Cache node contacts only; do not persist an unbounded torrent-to-peer database
  or reintroduce public discovery for private torrents.

**Acceptance:**

- Missing/stale/bad cache falls back to ordinary bootstrap without blocking it.
- DHT contacts are distinct from actual torrent peers in storage and UI.
- Cache size, TTL, malformed-input bounds and network-change behavior are explicit.
- No changes to private-torrent discovery restrictions or peer-selection policy.

**Verification when authorized:** local responsive/unresponsive nodes, expiry,
corruption, IPv4/IPv6 handling and fallback when every saved contact is unavailable.

## CACHE-04 — Bounded verified startup/resume reuse

**Dependencies:** REL-01 and CACHE-07. Build on current startup/RAM caches;
do not retune buffering, increase connection limits or create full video files.

**Likely files:** `engine-go/direct_storage.go`, `playback_resume.go`, existing
startup-cache validation/cleanup and Android cache settings.

**Work:**

- Add a small, explicitly bounded persistent window of already verified bytes
  around a recently useful resume read, alongside existing startup pieces.
- Reuse available verified data; persistence alone must not start a background
  transfer. Preserve external-player timestamp/seek ownership.
- Key ranges by torrent/file identity and piece layout. Validate restored data,
  reject partial/corrupt writes and coordinate replacement with active readers.
- Account for per-torrent and total disk usage; define episode switching, entry
  deletion, clear-cache behavior and inactive eviction before adding persistence.

**Acceptance:**

- Warm reopen can reuse valid bytes; unavailable/corrupt bytes follow ordinary
  verified fetching. Saved bookmarks never imply those bytes are still cached.
- Direct Play remains bounded and creates no full-sized media backing file.
- Cache cleanup cannot evict pinned active data or restore a manually paused session.
- No claim of smooth resume when peers/required uncached pieces are unavailable.

**Verification when authorized:** restart, corruption, atomic replacement,
piece-size boundaries, episode change, eviction, pause and delete during persistence.

## CACHE-05 — Reusable video information and preview locations

**Dependency:** MEDIA-01. **Likely files:** `ThumbnailRepository.kt`, saved
media-facts records, details UI consuming available media information.

**Work:**

- Cache successfully read duration/dimensions and timestamps that produced a
  usable real preview. Reuse these when selecting later extraction attempts.
- Key by immutable torrent/file identity and extraction/schema version rather
  than URI. Invalidate incompatible records or proven changed local media.
- Do not persist transient decoder failure as permanent incompatibility.

**Acceptance:**

- Cache hits avoid unnecessary repeated probing but never substitute metadata
  for verified payload or guarantee that the current device can decode a video.
- Unknown facts remain unknown. A preview timestamp is not a playback bookmark;
  only supported external-player results update watch progress.
- Size/entry bounds, cleanup ownership and safe corruption fallback are defined.

**Verification when authorized:** stable identity across URI changes, parser
version changes, missing facts, malformed records and transient extraction failures.

## CACHE-06 — Navigation continuity and static theme-rendering reuse

**Dependencies:** UI-01 and UI-02. **Likely files:** `LibraryScreen.kt`,
`TorrentFilesScreen.kt`, `MainActivity.kt`, theme drawing helpers.

**Work:**

- Preserve search/filter/sort, valid open-entry keys and scroll positions using
  appropriate Android saved state. Avoid persisting unnecessary work on each frame.
- Reuse static botanical/celestial artwork, glass decoration and pixel patterns.
  Key rendering by theme, appearance, size/density and other relevant inputs.
- Keep imagery bounded and release superseded drawings/bitmaps. Live transfer
  telemetry remains live; reduced effects still disables nonessential motion.

**Acceptance:**

- Recreation/restart restores useful context and safely handles removed entries.
- Never restore a pending delete confirmation or replay a destructive command.
- Theme/density/size/font changes do not display stale decoration or clip content.
- No unbounded full-screen bitmap cache or fake frozen peer/download status.

**Verification when authorized:** recreation, search/filter changes, deleted
entries, large text, theme/appearance changes and repeated screen navigation.

## CACHE-07 — Unified cache accounting and safe cleanup

**Dependency:** MEDIA-01. **Likely files:** `DownloadStorage.kt`,
`LibrarySession.kt`, `ThumbnailRepository.kt`, `SettingsScreen.kt`, Go cache stats
and existing cleanup operations.

**Starting point:** temporary-byte accounting includes thumbnail files, but the
current cleanup path leaves them behind. `ThumbnailRepository.clear()` cancels
its extraction scope, so it cannot be reused unchanged as a normal clear action.

**Work:**

- Report memory/disk usage for thumbnails, temporary imports, startup/resume
  pieces and the new peer/DHT/media-facts caches without double counting.
- Define per-type limits and aggregate pressure behavior. Evict eligible inactive
  entries or decline optional cache writes; preserve active readers/verification.
- Provide clear controls by category and a clearly scoped Clear temporary cache.
  Keep library records, bookmarks, preferences and downloaded files separate.
- Make thumbnail clearing reusable: cancel/invalidate old work safely, prevent
  it from republishing cleared images and permit later extraction to run normally.
- Coordinate entry deletion, concurrent cache writes and low-space failures.

**Acceptance:**

- Displayed reclaimed space corresponds to categories actually cleared.
- Clearing cache neither deletes user media nor changes selections/transfer intent.
- Active playback remains usable and cache owners can recover after clearing.
- A full disk/corrupt cache causes a bounded failure or cache miss, without
  poisoning durable state. Document soft targets versus enforceable hard limits.

**Verification when authorized:** clear during extraction/playback, late writer,
repeated clear/use cycles, entry deletion, accounting, low space and absent files.

## QA-01 — Local validation and handoff

**Work:**

- Run focused regressions as each behavior changes, then existing Go and Android
  suites and a debug APK build. Verify no embedded player dependency appears.
- Inspect all three themes and major flows on an available Android device or
  emulator only after implementation is authorized.
- Check automatic thumbnail attempts over both Wi-Fi and mobile, cached restart,
  offline/paused states, peer diagram motion and external-player launch/resume.

**Acceptance:**

- Record actual commands/results and any unavailable device checks below.
- Produce a distinct local debug APK and checksum; keep source work in local commits.
- Document actual device/codec limitations and remaining work. Passing unit tests
  alone does not establish that real Direct Play frames decode on a phone.
- For later cache milestones, record each ticket's targeted correctness results
  and cleanup/restart behavior. Existing UI/thumbnail handoff need not wait for
  every independent cache enhancement. NET-01 effectiveness remains user-tested.

## STORAGE-01 — Later storage recovery

**State:** backlog; separate from the first redesign.

- Improve destination/free-space information and explain access failures.
- Design non-destructive recovery for moved files or lost folder permission.
- Never truncate, delete or redownload existing media merely to repair access.
- Define provider-specific and restart tests before claiming recovery is complete.

## Proposed order after an explicit start instruction

1. Investigate/fix REL-01 for reliable checkpoint persistence. Record evidence;
   do not expand trusted completion or assume a reproduced disk failure.
2. UI-01, then UI-02/UI-03/UI-04 in bounded local commits; CACHE-06 accompanies
   the relevant screens rather than duplicating the theme/navigation work.
3. MEDIA-01 and MEDIA-02, with CACHE-07 cleanup/accounting and CACHE-05 reusable
   media facts where they share the thumbnail lifecycle. Run QA-01 for that delivery.
4. CACHE-01/CACHE-02/CACHE-03 are separate recovery/discovery increments. Add
   CACHE-04 after its durability/accounting dependencies, without holding earlier
   completed UI work for all cache additions. STORAGE-01 stays a separate milestone.
5. NET-01 is independently deliverable as an experimental trial; no
   ISP-effectiveness test milestone blocks handing it to the user once
   implementation is authorized.

## Execution log

- Planning: current app already committed at `a226754`; working tree was clean.
- Agreed scope recorded. Broader adaptive/evolutionary engine work explicitly excluded.
- Before the user's instruction to defer implementation, baseline checks completed:
  `go test ./...` in `engine-go` passed; `./gradlew :app:testDebugUnitTest :core:test
  --console=plain` in `android` passed. These verify only the unchanged baseline.
- User explicitly deferred implementation. No application/native source changed;
  no redesigned APK was built or installed. All implementation tickets remain Planned.
- Added NET-01 at the user's request: built-in anti-DPI Settings switch, Off by
  default, with effectiveness testing left to the user. Documentation only.
- User requested all eight cache improvements discussed in review. Extended
  MEDIA-01 and added CACHE-01–07 plus REL-01, including dependencies, invalidation,
  cleanup and recovery acceptance criteria. Implementation remains on hold.

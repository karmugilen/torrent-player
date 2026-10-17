# Local redesign agreement

Baseline: `a226754`. Decisions agreed in the local design conversation on 2026-10-17.

**Implementation is now authorized.** On 2026-10-17 the user explicitly invoked
Herdr and implement and requested agy and grok to complete the work in parallel.
This supersedes the earlier instruction to wait. Keep delivery local and use
completion reports/callbacks instead of repeated status polling. See
[REDESIGN_EXECUTION.md](REDESIGN_EXECUTION.md) for ownership and integration.

## Delivery and scope

- Work locally. Track tickets in Markdown and make local commits. Do not push,
  publish, open GitHub issues, or change the upstream branch history.
- External Android video players are mandatory. Do not introduce an embedded player.
- Do not implement the proposed adaptive/evolutionary architecture work, change
  peer-selection algorithms, increase connection limits, or retune buffering.
- Narrow engine work needed for safe thumbnail reads is in scope; retain the
  existing scheduler, transfer intent, verification, storage, and playback behavior.
- A separately planned built-in anti-DPI switch is included for the user's own
  network trials. This does not reopen the rejected adaptive/evolutionary work.
- The user also requested all eight reviewed cache/recovery improvements and the
  checkpoint persistence fix. These are included in the authorized work.

## Visual direction

Three selectable themes share one set of screens and controls:

1. **Aero** (default): blue/teal water, translucent glass, gloss, highlights and
   tactile depth inspired by Vista/Windows 7.
2. **Antiquity**: parchment, botanical line work, celestial ornament, warm brass
   and serif headings inspired by Art Nouveau and historic prints.
3. **Retro**: classic window framing, bevels, pixel details and restrained
   monospace typography inspired by 1980s/1990s computing.

Rich, mostly still backgrounds; short interaction transitions; a reduced-effects
setting. Maintain legible text, accessible touch targets, font scaling and light
and dark appearances. Wallpaper-derived dynamic colors must not erase each
theme's identity.

## User experience

- A watch-first library: automatic thumbnails, Continue watching when a compatible
  external player supplies a bookmark, clear transfer status and an obvious Add action.
- Expose existing stable sorting and add meaningful filters and batch controls.
- Put playable files and episode controls ahead of technical diagnostics.
- Improve episode browsing, file search and matching subtitle selection.
- Keep existing per-file controls, saved data, magnet intake, Direct Play and
  external-player contracts working.
- Storage recovery is a separate later ticket, not part of the first redesign.

## Peer visualization

Preserve the existing geographic world map. Replace the circular reactor with
a stable vertical branching connection view: device, TCP/uTP/WebRTC groups,
and rectangular expandable peer tiles. Show actual measured transfer activity;
idle connections must not imply data transfer. Unknown countries remain unknown.
Large swarms are summarized and expandable. Reduced effects disables flow motion.

## Automatic thumbnails

- Automatic on Wi-Fi and mobile data. No manual Load preview gate.
- Automatic thumbnail loading is a required feature, not an optional stretch goal.
- Attempt a real decoded video frame before playback/full download, including
  Direct Play. Reuse already available verified bytes before fetching more.
- Cache successful images persistently for incomplete and Direct Play entries,
  keyed by torrent/file identity; display cached images promptly after restart.
- One extraction at a time. Preview reads have explicit time and piece-aware
  byte limits, cancellation and cleanup; no unlimited background fetch.
- Explicit pause/stop/delete and existing transfer restrictions remain effective.
  A thumbnail read must not start a full download or change playback focus/history.
- Retry transient failures with bounded backoff as conditions change. Avoid
  permanent failure markers that prevent ordinary recovery.
- Try independent decode fallbacks and accept a dark real frame as a last resort.
- Missing metadata/peers, unsupported codecs or corrupt media cannot guarantee a
  decoded image. Show an honest themed loading/unavailable state in those cases.

## Cache and recovery improvements

All eight reviewed areas are included. Persistent thumbnail work extends
MEDIA-01; CACHE-01–07 cover the remaining areas, and REL-01 covers checkpoint
durability. See the board for dependencies and acceptance criteria.

1. Persist successful incomplete/Direct Play thumbnails and load them independently
   of current engine/URI availability; retain bounded automatic retry.
2. Keep bounded last-known-good library recovery snapshots and versioned per-torrent
   metadata, preserving migration, explicit removal and user intent.
3. Persist short-lived, bounded recently successful public-torrent peer hints.
   Revalidate them through the current route; never count them as connected peers.
4. Keep bounded responsive DHT bootstrap contacts with aging, revalidation and
   normal bootstrap fallback, preserving private-torrent restrictions.
5. Add a small verified persistent startup/resume window, after durability and
   cache accounting are reliable. Do not retune buffering or create full videos.
6. Cache valid duration/dimensions and successful preview timestamps by immutable
   media identity; never treat preview positions as external-player bookmarks.
7. Restore useful navigation state and reuse static theme decoration with correct
   invalidation, bounded memory and reduced-effects support.
8. Unify cache sizes/limits and clearing by category while protecting active
   playback, downloaded files and durable library/bookmark state.

Prioritize checkpoint save reliability: a code-review finding showed ignored
sync/write errors followed by clearing dirty state. Investigate and fix that
path without claiming an observed failure or broadening checkpoint trust.

Critical user state belongs in durable storage. Regenerable caches may be evicted;
missing, corrupt or expired entries must fall back safely. Existing tracker-list,
metadata and startup caches remain starting points, not duplicate new layers.
No additional Rust migration, evolutionary/adaptive controller or peer scheduler
is selected by this cache scope.

## Built-in anti-DPI: user trial

- Add **Settings → Network → Anti-DPI (experimental)**, with a persisted On/Off
  switch, **Off by default**. The user wants zapret-style behavior built into
  the app, rather than being required to run a separate tool.
- On must activate a real supported bypass implementation, not merely save a
  preference or rename the engine's existing handshake obfuscation.
- Off restores normal networking and releases bypass-specific resources. Changes
  must not delete saved data, change file selection, or break external playback.
- Select the smallest viable Android integration when implementation is later
  authorized. Do not assume packet-level desktop zapret can simply be bundled
  into an ordinary Android app. Clearly identify the techniques/transports
  actually supported by the delivered trial build.
- This is an experimental connectivity feature. Do not label it anonymous,
  invisible to the ISP, or guaranteed to find additional peers.
- The user will test effectiveness and report feedback. **Do not require an
  upfront ISP test, packet capture, public-swarm benchmark, or demonstrated bypass
  success before providing the functional experimental switch.** Do not conduct
  live ISP/network effectiveness testing on the user's behalf.
- Whether failed adapted connections may retry untreated, and what happens to
  transports unsupported by the chosen backend, must be documented explicitly.
  Do not silently present ordinary connections as bypassed.
- Rust helper candidates remain an investigation; no additional Rust migration
  has been selected or authorized for implementation.

## Explicitly excluded

- Adaptive/evolutionary architecture work, genetic optimization, a new peer
  scoring/scheduling layer, and connection or buffering retuning.
- An embedded player, replacing the geographic map, GitHub issues, pushing or
  publishing. The previous implementation-start hold has been superseded.

## Validation

Run focused tests for preview cancellation/budgets/persistence and library actions;
run existing Go and Android suites and build a local debug APK. Inspect the UI on
an available Android device/emulator when possible. Report device/codec limits
honestly. Measure engine read behavior rather than claiming an external player's
rendered first-frame latency. Do not claim network speedups from visual changes.
For the anti-DPI trial, effectiveness testing belongs to the user as specified
above; a working local build is not evidence of ISP bypass.

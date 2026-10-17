# Local redesign agreement

Baseline: `a226754`. Decisions agreed in the local design conversation on 2026-10-17.

**Implementation is not authorized to start yet.** The user's latest instruction
is: "implementation will start when I tell you." This document and the ticket
board are planning artifacts only. Wait for an explicit instruction to implement.

## Delivery and scope

- Work locally. Track tickets in Markdown and make local commits. Do not push,
  publish, open GitHub issues, or change the upstream branch history.
- External Android video players are mandatory. Do not introduce an embedded player.
- Do not implement the proposed adaptive/evolutionary architecture work, change
  peer-selection algorithms, increase connection limits, or retune buffering.
- Narrow engine work needed for safe thumbnail reads is in scope; retain the
  existing scheduler, transfer intent, verification, storage, and playback behavior.

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

## Explicitly excluded

- Adaptive/evolutionary architecture work, genetic optimization, a new peer
  scoring/scheduling layer, and connection or buffering retuning.
- An embedded player, replacing the geographic map, GitHub issues, pushing or
  publishing, and starting implementation before the user requests it.

## Validation

Run focused tests for preview cancellation/budgets/persistence and library actions;
run existing Go and Android suites and build a local debug APK. Inspect the UI on
an available Android device/emulator when possible. Report device/codec limits
honestly. Measure engine read behavior rather than claiming an external player's
rendered first-frame latency. Do not claim network speedups from visual changes.

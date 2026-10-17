# Torrent Player 2.1.1 — Material UI and playback fixes

This update combines the Android redesign and native engine improvements. Work,
tickets, commits and APKs remain local. External Android players are still the
required playback path.

## Appearance and UI consistency

- **Dynamic Material You colours** are enabled by default on Android 12 and
  newer and follow the wallpaper palette. The choice survives restart.
- Solid Material surfaces, standard typography, controls and shapes throughout
  Library, Add, Prepare, Details and Settings. Glass, transparent panels, gloss,
  ornaments and decorative screen textures have been removed.
- Light/dark appearance remains selectable. Aero, Antiquity and Retro are optional
  solid colour palettes when dynamic colours are disabled; older Android versions
  use the fallback palette.
- Add displays its own screen. Library controls and cards cannot show through it.
- Peer addresses occupy a separate, bounded line. IPv6 addresses cannot squeeze
  byte counters into a vertical column. Long labels are ellipsized consistently.
- Scrollable filters and saved navigation/scroll state remain.
- The Continue watching section has been removed as requested. Per-file resume
  positions still work with external players that support the result contract.

## Library and navigation

- All, Active, Ready and Attention filters, title/file search and stable sorting.
- Batch pause, resume and removal with keep-files and erase-files choices.
- Clear first-use, empty-search and empty-filter states.
- Returning from Settings or details restores the library position.

## Playback continuity and switching

- Switching Direct Play videos no longer removes another torrent's engine session,
  clears its cached progress, or forces it through restore again.
- Each external proxy descriptor owns a reference-counted playback lease. Opening
  a new movie or closing an old probe cannot release another player's lease.
- A completed download retains foreground process protection while a player is
  still reading its open proxy. The foreground service ends after the last stream
  descriptor closes when no transfers remain.
- Completion notifications are silent so their alerts do not interrupt audio focus.
- The native regression reads through the same handle before and after completion.
  Android regressions cover completion notification lifetime and switching leases.

## Adding torrents and playing files

- Clearer **Watch now / Direct Play** and **Save for offline** choices.
- Playable files appear ahead of transfer diagnostics.
- File search, natural episode ordering, video/subtitle filters and matching
  subtitle labels. Existing external-player playlists and subtitle grants remain.
- Real thumbnails are requested automatically in ready Add/Prepare drafts,
  the library and details, including Direct Play. No manual Load preview button
  or separate Wi-Fi-only thumbnail restriction was added.

## Thumbnails and preview reads

- During Direct Play, previews can refresh as the playback byte position and
  verified RAM-cache contents change, rather than permanently returning the first
  cached image. Refreshes are bounded to once per 12 seconds per item.
- Up to three real frames retain the initial preview and recent decoded frames;
  the latest successful Direct Play frame is displayed automatically.
- While a player has a stream open, thumbnail reads use verified cached data and
  do not add piece demand. Missing metadata/keyframes retain the previous image.
- Byte position is a best-effort hint for a frame timestamp, not exact watch
  progress. Available keyframes and device codecs determine which frame can decode.
- Extraction runs only while its screen is visible and cancels on screen exit.

- The first successful real frame is saved for incomplete and Direct Play items
  and reused after restart. Drafts and library entries share a stable identity.
- Seekable native reads let Android request container metadata at the head or
  tail without downloading a complete video or launching a player.
- One preview session at a time, a 10-second session deadline, and an 8 MiB
  whole-piece network admission budget. Already verified data can be reused.
- Closing a preview cancels blocked native reads, releases temporary storage and
  removes stale completion flags. Pause, delete and global payload policy apply.
- Decoder fallbacks, cached media dimensions/duration/preview timestamps,
  retry backoff, and a final dark-frame fallback.
- Disk decoding runs off the UI thread. Cache clearing invalidates late results,
  closes active preview handles, and allows fresh extraction afterward.

Thumbnails still require available video bytes and a decoder supported by the
phone. Missing peers or unsupported codecs produce a truthful placeholder.

## Peer connections

- The circular reactor is replaced by a vertical branching view: device,
  TCP/uTP/WebRTC groups, and expandable rectangular peer tiles.
- Stable peer ordering and activity derived from actual transfer counters.
- The geographic world map remains available.

## Experimental anti-DPI setting

- Settings includes a persisted On/Off switch, disabled by default.
- The native implementation splits initial writes on supported outbound TCP
  peer and HTTP/WebSocket tracker connections. It uses a per-client hook,
  resets tracker connections on toggles, and reports its actual coverage.
- Mode is reported as `tcp-write-split`. UDP, uTP and WebRTC are not covered.

This is an experimental userspace technique. It does not guarantee that TCP
packets remain split, increase peer counts, hide activity from an ISP or provide
anonymity. ISP effectiveness is for the user to test; no efficacy claim was made.

## Caches and reliability

| Area | Improvement |
| --- | --- |
| Thumbnails | Persistent real frames, bounded memory/disk use and reusable clearing. |
| Library/metadata | Atomic saves, bounded recovery snapshots, isolated metadata recovery and deletion generations that allow explicit re-adds. |
| Peer hints | Expiring successful public-torrent endpoints; unknown/private metadata is excluded. |
| DHT | Bounded, expiring bootstrap contacts with their real UDP ports; unconfirmed contacts remain unconfirmed. |
| Resume bytes | A small verified Direct Play resume window, atomically saved with metadata/checksum and checked against torrent piece hashes before reuse. |
| Media facts | Validated video dimensions, duration and reusable preview timestamps. |
| UI/rendering | Saved screen state and shared Material colour/shape resources. |
| Cleanup | Native category accounting and clearing, active-reader protection, visible failures and synchronized cache operations. |

Checkpoint sync/write failures retain dirty recovery work for retry. Native status
now exposes a non-fatal checkpoint warning, shown in torrent details. A warm resume
cache does not override an explicit Play from start action.

## Storage recovery

- Settings displays the selected destination and offers a folder picker, access
  regrant and reset to the default Downloads/Webtor location.
- Details offers **Locate existing file**. A replacement must be writable and
  have the expected size when the provider supplies it.
- Linking a replacement stops the old native session, preserves selection and
  bookmarks, and leaves the entry paused. Resume verifies the saved data before
  downloading; a matching filename or size alone is not treated as verification.

## Optimized release build and validation

The local ARM64 release uses R8 code optimization/minification, resource shrinking,
optimized Rust compilation and a stripped Go JNI library. It is not debuggable.
The existing local signing certificate is used so it can update this phone's app.

```sh
cd android
./gradlew :app:assembleRelease -Pwebtor.localSigning=true --console=plain
```

Validation includes the Go suite, focused native concurrency/race regressions,
`go vet`, Rust's 10 tests, Android app tests and the core's 23 tests. The final
delivery record, APK checksum and device verification are recorded in
[docs/REDESIGN_DELIVERY.md](docs/REDESIGN_DELIVERY.md).

The app's peer scheduler, connection limits and playback buffer policy were not
replaced with an adaptive/game-theory system. Existing Rust helpers remain in use;
this update does not claim an additional Rust migration.

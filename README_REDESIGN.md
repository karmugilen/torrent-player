# Torrent Player 2.1.0 — local redesign

This update combines the Android redesign and native engine improvements. Work,
tickets, commits and APKs remain local. External Android players are still the
required playback path.

## Appearance

- **Aero, the default:** translucent glass panels, glossy controls, water-like
  gradients and layered wave highlights.
- **Antiquity:** parchment tones and paper fibres, botanical leaves, celestial
  constellations, brass details and serif typography.
- **Retro:** classic window colours, bevelled edges, a stippled desktop and
  monospace typography.
- Themes share the same features, support light/dark appearance, and persist
  across restarts. Reduced effects removes decorative treatments.
- Shared screen backdrops remain visible behind the main screens. Static
  decorations are cached instead of generating new artwork on every draw.

## Library and navigation

- Continue watching opens the actual bookmarked episode in an external player.
  The most recently updated bookmark wins; finished episodes are excluded.
- All, Active, Ready and Attention filters, file/title search, and stable sorting.
- Batch pause, resume and removal with separate keep-files and erase-files choices.
- Clearer empty/search states and horizontally scrollable filters on small screens.
- Saved navigation and screen state retain the library position when returning
  from Settings or a torrent's details.

## Adding torrents and playing files

- Clearer **Watch now / Direct Play** and **Save for offline** choices.
- Playable files appear ahead of transfer diagnostics.
- File search, natural episode ordering, video/subtitle filters and matching
  subtitle labels. Existing external-player playlists and subtitle grants remain.
- Real thumbnails are requested automatically in ready Add/Prepare drafts,
  the library and details, including Direct Play. No manual Load preview button
  or separate Wi-Fi-only thumbnail restriction was added.

## Thumbnails and preview reads

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
| UI/rendering | Saved screen state and cached static theme decoration. |
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

## Release build and validation

The local ARM64 release uses R8 code optimization/minification, resource shrinking,
optimized Rust compilation and a stripped Go JNI library. It is not debuggable.
The existing local signing certificate is used so it can update this phone's app.

```sh
cd android
./gradlew :app:assembleRelease -Pwebtor.localSigning=true --console=plain
```

Validation includes the Go suite, focused native concurrency/race regressions,
`go vet`, Rust's 10 tests, Android app tests and the core's 22 tests. The final
delivery record, APK checksum and device verification are recorded in
[docs/REDESIGN_DELIVERY.md](docs/REDESIGN_DELIVERY.md).

The app's peer scheduler, connection limits and playback buffer policy were not
replaced with an adaptive/game-theory system. Existing Rust helpers remain in use;
this update does not claim an additional Rust migration.

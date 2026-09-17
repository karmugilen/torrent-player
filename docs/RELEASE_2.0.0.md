# Torrent Player 2.0.0 (23)

Major update on the Go engine branch: faster video play/seek, safer magnet
restore after force-stop, correct per-torrent speeds, and clearer torrent
details. Install over the existing APK; saved downloads are reused.

## What's new

### Playback and download path
- MediaStore download path with RAM write-back and coalesced disk sync.
- Live progress and pieces heat map on torrent details.
- Piece-grid file-name labels removed for a cleaner details layout.
- Transfer speed-cap controls removed from the UI.

### Faster Play / seek for videos
- Gated head+tail moov tip while opening so moov-at-end MP4s start sooner.
- Soft moov prepare: head/tail tips stay urgent while the middle of the file
  keeps downloading.
- Scrub debounce and full seek readahead applied immediately while scrubbing.
- Seek hotspots dropped to avoid thrash during open/scrub.

### Magnet restore and share
- Metadata-pending magnets keep connecting after force-stop / app restart.
- Pending magnets restore through resume without requiring local storage first.
- Details screen shows **Share magnet** and **Copy link** on the summary card.

### Speeds, peers, and notifications
- Own transfer rates per torrent so library cards and notifications no longer
  share one engine-wide speed.
- Combined notification reflects the active torrent correctly.
- Play-path write-back reload after cache eviction avoids zero-clobber of
  flushed chunks (hash-fail loop around ~37–60MB).

### Carried forward from 1.4.5
- Background saved-data verification without the old 60s control deadline.
- Library/notification checking progress; pause/resume/remove cancel cleanly.

## Package

- APK: `torrent-player-2.0.0.apk` (arm64-v8a, version code 23, version name 2.0.0).
- Optimized release build signed with the dedicated **Torrent Player release
  keystore** (`CN=Torrent Player`), not the Android Debug key. See
  [SIGNING.md](SIGNING.md).
- If you previously installed a debug-signed 1.4.x / early 2.0.0 build, uninstall
  once before installing this APK (signature change). Future updates signed with
  this same release key install over it normally.
- Branch: `go-engine`. Tag: `v2.0.0`.
- Play Protect may still show a scan prompt for sideloaded (non–Play Store)
  installs; that is expected and not an unsigned-APK failure.

## Changes since `v1.4.5`

- Ship download-path playback stack: RAM write-back, live progress, heat grid.
- Speed up play/seek with gated moov tip and scrub debounce.
- Prepare moov before mid-file on selected video downloads.
- Prefer soft moov prepare: tips Now while mid still downloads.
- Fix play download thrash and magnet restore/share.
- Fix per-torrent speeds, combined notification, and pending resume.
- Surface Share magnet / Copy link on torrent details; drop piece-grid file labels.
- Bump version to 2.0.0 (23).

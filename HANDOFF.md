# Torrent Player — agent handoff overlay

Copy this file (or the repo) to the next agent. Work from `master` on GitHub, not from chat memory.

## Identity

- Display name: **Torrent Player**
- Application id: `webtor.app` (do not change; that would be a different install)
- Files on disk: `Downloads/Webtor/` (keep this path; renaming it orphans existing files)
- GitHub: https://github.com/karmugilen/torrent-player
- Latest release: **v1.2.2** — https://github.com/karmugilen/torrent-player/releases/tag/v1.2.2
- Release source: `v1.2.2` on `origin/master`
- versionName `1.2.2` / versionCode `5` in `android/app/build.gradle.kts`
- APK ~18 MB (was ~53 MB). Keep size wins.

Use only with torrents the user has the right to download. Sintel is the CC fixture.

## What it is

Android torrent client: paste magnet → connect peers immediately (prefetch) → pick files → write to MediaStore `Downloads/Webtor` → play in an external player (VLC/mpv/etc) via local HTTP stream while still downloading.

Engine is Node 18 (nodejs-mobile) running `webtorrent@2.2.1` with no native uTP/WebRTC. Kotlin talks to it on `127.0.0.1`.

## Layout

```
engine/                 WebTorrent daemon (main.js HTTP control + stream)
engine/document-store.js  Android FD store + in-memory prefetch cache (48MB cap)
engine/protocol.js      JSON views, default video selection
android/app/            Compose UI (webtor.app)
android/core/           EngineClient HTTP client
scripts/sync-engine.sh  Prune + copy engine into APK assets
scripts/vendor-node.sh  Unpack nodejs-mobile, strip libnode.so
android/app/icon/play-mark.svg  Launcher glyph source
```

Gitignored (regenerated on build): `engine/node_modules`, `android/app/src/main/assets/nodejs-project`, `jniLibs`, `cpp/include`, vendor zip/unpacked, `android/SDK.env`, APKs.

## Build / test / install

```
cd engine && npm ci --omit=optional && npm test
source android/SDK.env
./scripts/vendor-node.sh          # once per machine; needs vendor zip
cd android && ./gradlew :core:test :app:testDebugUnitTest :app:assembleRelease
adb uninstall webtor.app || true  # needed if launcher caches icons
adb install -r app/build/outputs/apk/release/app-release.apk
```

Phone used in this project: `adb devices` → TetrisIND A015 (`00121649A004784`).

Release signing uses the **debug** keystore (`signingConfig = debug`). Bump **both** `versionCode` and `versionName` before a GitHub release. Tag `vX.Y.Z` and attach `torrent-player-X.Y.Z.apk`.

`sync-engine.sh` strips maps/docs/tests, `webtorrent/dist`, `xml2js.bc.js`, `prebuilds`, `*.bare`. Do not put those back unless a test proves they are required.

## Architecture (do not fight this)

1. **Paste magnet** (`LibrarySession.setMagnet`, 180ms debounce) → `POST /add` with `prepare: true` → peers + metadata start. Stay on add sheet until Add.
2. **Add** → Prepare screen, same engine id. Do not re-add.
3. **Back / close sheet** (uncommitted) → `DELETE` torrent `destroyStore: true`, drop prefetch cache. No files in Downloads.
4. **Download** → Kotlin opens MediaStore FDs → `POST /configure` with descriptors + selected indexes → DocumentStore attach + flush cache → rescan → continue same swarm.
5. **Play** → completed file content URI, otherwise `POST /play` HTTP stream; launch default player package if set, else system chooser.
6. Content files must not be created under Downloads until the user taps Download. Prefetch is RAM only (`DocumentStore` cache, pause select when full).

Control API (engine/main.js): `/add` `/torrent/:id` `/play` `/pause` `/resume` `/remove` `/configure` `/select` `/metadata` `/settings` `/stats`.

## UI rules

- Dark is default (`UiState.darkTheme = true`, pref `darkTheme`). Light is a Settings toggle. Colors go through `MaterialTheme.colorScheme` only.
- Minimal / editorial: bone-or-night canvas, hairline dividers, serif titles, monospace sizes, no FAB, no Material icons-extended, no navy+mint.
- Launcher: adaptive icon background `#161513`, foreground from density/nodpi PNGs generated from `android/app/icon/play-mark.svg`. Never ship a 1024px `res/drawable/ic_launcher_foreground.png` — that made OEMs show the Android robot. Uninstall before reinstall when changing icons.

## Shipped (do not re-do)

- Magnet paste prefetch + continue on Start + clear on back/close
- Default player picker in Settings
- Dark/light appearance
- Rename Gale → Torrent Player (package id still `webtor.app`)
- Vector play-mark icon, v1.2.1
- Size cut: strip libnode, prune engine assets, R8 + shrinkResources, drop `material-icons-extended`

## Fixes in 1.2.2 / versionCode 5

This release includes the fixes below; do not repeat the old peer-status-only task.

- Reset previous peer tuning once to WebTorrent's default 55; restore default NAT
  discovery. Keep uTP/WebRTC disabled because this Android runtime lacks them.
  DHT/tracker discovery and retry timing remain WebTorrent defaults.
- Keep draft peer status visible after metadata arrives with zero peers.
- Report file completion from verified `file.done`, working around the installed
  version's exact-piece-boundary progress undercount. Torrent completion in the
  control API is derived from selected file completion.
- Rebuild selections after attaching storage/rescanning; preserve an explicitly
  empty selection in the Prepare UI.
- Completed Play uses the saved content URI before any engine restore/resume.
  Automatic foreground-service teardown no longer invokes user Stop.
- Tap a library torrent to open its selected files. Multi-file library action is
  Files; each file row has Play targeting that exact index. Complete files play
  locally; partial files stream through the engine.
- Remove closes the engine store first, then deletes file records and empty
  ancestors strictly below Downloads/Webtor. Persist relative paths for retries;
  old records obtain paths from MediaStore before deletion. Never recursively
  delete folders or remove unrelated contents.
- Disk scans/file-access checks run on IO. Poll less frequently, avoid duplicate
  draft polling, throttle foreground updates, and animate progress indicators.

Validation:
- Engine: 24 tests, including three video files exceeding the 48 MiB cache,
  byte comparisons, disk restore, and exact-boundary completion.
- Android: 11 core tests and 15 app tests passed; release assembly and vital lint passed.
- Phone 00121649A004784: generated two-video fixture downloaded byte-for-byte,
  first-tap local playback passed, playback after app restart kept engine torrent
  count at zero, and Delete files removed both nested and group directories.
- Phone: per-file screen showed both clips; episode 2 Play launched its own content URI, verified against the MediaStore display name.
- Upgrade with `adb install -r`; do not uninstall or clear user data for these fixes.

## Conventions

- Kotlin app code under `android/app/src/main/java/webtor/app/`
- Engine tests: `engine/test/*.test.js` (`node --test`)
- Bare magnets get `udp://tracker.opentrackr.org:1337/announce` if the magnet has no announce list
- Readable errors; no silent failures
- Do not commit `node_modules`, `jniLibs`, vendor zip, `.grok`, `SDK.env`, APKs, `full1024.png`

## Next agent first commands

```
git fetch origin && git checkout master && git pull
git log -5 --oneline
source android/SDK.env && adb devices
```

Then implement the peer-status/retry item above. Do not rename the app, do not change `applicationId`, do not revert the size or icon work.

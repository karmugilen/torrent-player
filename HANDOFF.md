# Torrent Player — agent handoff overlay

Copy this file (or the repo) to the next agent. Work from `master` on GitHub, not from chat memory.

## Identity

- Display name: **Torrent Player**
- Application id: `webtor.app` (do not change; that would be a different install)
- Files on disk: `Downloads/Webtor/` (keep this path; renaming it orphans existing files)
- GitHub: https://github.com/karmugilen/torrent-player
- Latest release: **v1.2.1** — https://github.com/karmugilen/torrent-player/releases/tag/v1.2.1
- HEAD: `adadd78` on `origin/master`
- versionName `1.2.1` / versionCode `4` in `android/app/build.gradle.kts`
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
5. **Play** → `GET /play` HTTP stream; launch default player package if set, else system chooser.
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

## Open work (last unfinished request)

**Peer connecting status / retry.** User: connecting-to-peer used to show, then vanished somewhere in the flow; also retrying.

Likely bug in `android/app/src/main/java/webtor/app/Formatters.kt` `prefetchStatus`:

```
ready && downloaded > 0 → "X already downloading · N peers"
peers > 0 → "Connecting · N peers"
ready → null          // hides the line after metadata, before bytes
else → "Finding peers…"
```

Used by AddSheet and PrepareScreen. After metadata is ready with 0 bytes the connecting line goes blank.

Wanted:

- Finding metadata → `Finding peers…` (+ thin bar on Add)
- `numPeers > 0` → `Connecting · N peers` even after ready
- Bytes in → already downloading line
- 0 peers: keep showing finding/retrying, do not blank; optional reannounce/DHT retry without destroying the torrent
- Back/close still clears prefetch; Download still continues the same engine id

Verify with `cd engine && npm test` (22 tests) and `:app:testDebugUnitTest`. Add a unit test that `prefetchStatus` is never null while a draft exists. If APK changes, bump to **1.2.2 / versionCode 5**, `assembleRelease`, `gh release create v1.2.2`.

An agent was started on this and **cancelled** — it did not land a commit.

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

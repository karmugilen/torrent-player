# Torrent Player

Android torrent downloader: magnet → pick files → `Downloads/Webtor` → your video player.

Paste a magnet and it starts finding peers immediately. Pick files, tap Download, play through VLC, mpv, or another installed player while the file is still coming in.

Engine is `webtorrent@2.2.1` (no native uTP/WebRTC addons) on nodejs-mobile Node 18.20.4. The app talks to it on `127.0.0.1`.

Use this only with torrents you have the right to download. Sintel is the CC test fixture.

## Install

Grab the APK from [Releases](https://github.com/karmugilen/torrent-player/releases).

```
adb install -r torrent-player.apk
```

## Build

```
cd engine && npm ci --omit=optional
source android/SDK.env
./scripts/vendor-node.sh
cd android && ./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```

`assembleRelease` is minified and resource-shrunk. Debug builds work the same without R8.

## Using the app

Paste a magnet or open a `.torrent` file, review files, then download. Files go to `Downloads/Webtor`. A fresh install does not ask for broad storage permission.

Pause stops peers and keeps the files. Delete asks whether to keep or erase them. Play uses your default player if you set one in Settings.

Bare magnets use DHT plus the public tracker from [OpenTrackr](https://www.opentrackr.org/). A magnet with no reachable seeds cannot download metadata or play.

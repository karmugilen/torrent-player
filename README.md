# Torrent Player

Android torrent downloader: magnet → pick files → `Downloads/Webtor` → your video player.

Paste a magnet and it starts finding peers immediately. Pick files and tap Download to save them, or use Watch now to stream the selected video through a bounded 100 MiB memory cache without saving it to the phone.

Engine is `webtorrent@2.2.1` on nodejs-mobile Node 18.20.4 with native uTP and WebRTC data channels. The app talks to it on `127.0.0.1`.

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
./scripts/build-native-addons.sh
cd android && ./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```

`assembleRelease` is minified and resource-shrunk. Debug builds work the same without R8.

## Using the app

Paste a magnet or open a `.torrent` file, review files, then download. Files go to `Downloads/Webtor`. A fresh install does not ask for broad storage permission.

Pause stops peers and keeps the files. Delete asks whether to keep or erase them. Play and Watch now use Android's native app picker, where you can choose a player just once or make it the default. An active download notification cannot be swiped away and offers one Stop action; Stop pauses the transfers so they can be resumed from the app.

Bare magnets use DHT, UDP trackers (OpenTrackr and others), and WebTorrent WebSocket trackers. Peers connect over TCP, uTP, and WebRTC. A magnet with no reachable seeds cannot download metadata or play.

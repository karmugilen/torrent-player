# Torrent Player

Android torrent downloader: magnet → pick files → `Downloads/Webtor` → your video player.

Paste a magnet, choose files, and tap Download. Play opens your Android media player while the files are downloading or after they finish. Multi-video torrents let you choose an episode from Details. The interface uses Material 3 components, clear transfer controls, and light/dark themes.

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

Paste a magnet or open a `.torrent` file, review files, then tap Download. Files go to `Downloads/Webtor`, and downloads stay in your library across app restarts. Previously saved metadata-only entries remain paused until you explicitly choose Download. A fresh install does not ask for broad storage permission.

Pause stops peers and keeps the files. Delete asks whether to keep or erase them. Play uses Android's native app picker, where you can choose a player just once or make it the default. Active notifications are marked ongoing and refreshed after dismissal on Android versions that permit swiping them. They offer one Stop action, which pauses downloads so they can be resumed from the app.

Bare magnets use DHT, UDP trackers (OpenTrackr and others), and WebTorrent WebSocket trackers. Peers connect over TCP, uTP, and WebRTC. A magnet with no reachable seeds cannot download metadata or play.

# Torrent Player

<p align="center">
  <img src="docs/images/icon.png" width="80" height="80" alt="Torrent Player Icon" />
  <h2 align="center">Torrent Player</h2>
  <p align="center">
    <strong>Fast, native, battery-friendly torrent downloader and streaming player for Android.</strong>
  </p>
  <p align="center">
    <a href="https://developer.android.com"><img src="https://img.shields.io/badge/Android-10%2B%20(API%2029%2B)-3DDC84?style=flat-square&logo=android&logoColor=white" alt="Android 10+ (API 29+)"></a>
    <a href="https://m3.material.io"><img src="https://img.shields.io/badge/Material%20Design-3-795548?style=flat-square&logo=materialdesign&logoColor=white" alt="Material Design 3"></a>
    <a href="https://webtorrent.io"><img src="https://img.shields.io/badge/WebTorrent-2.2.1-brightgreen?style=flat-square&logo=webtorrent&logoColor=white" alt="WebTorrent"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square" alt="License: MIT"></a>
    <a href="https://github.com/karmugilen/torrent-player/releases"><img src="https://img.shields.io/github/v/release/karmugilen/torrent-player?style=flat-square&color=orange" alt="Release"></a>
  </p>
</p>

---

## Screenshots

<p align="center">
  <img src="docs/images/library.png" width="31%" alt="Library & Video Previews" />&nbsp;
  <img src="docs/images/add_sheet.png" width="31%" alt="Add Magnet & Select Files" />&nbsp;
  <img src="docs/images/details.png" width="31%" alt="Details, Telemetry & Pieces" />
</p>
<p align="center">
  <sub><b>Left:</b> Library & Video Previews &nbsp;|&nbsp; <b>Center:</b> Add Magnet & File Selection &nbsp;|&nbsp; <b>Right:</b> Details, Swarm Telemetry & Pieces</sub>
</p>

---

## Key Features

- **Single-Contract Workflow**: Straightforward pipeline: **Magnet / Torrent $\to$ Local Storage (`Downloads/Webtor`) $\to$ Stream / Play**. No confusing virtual containers or temporary caches.
- **In-Progress Streaming & Per-File Playback**: Launch directly into your favorite Android video player (VLC, Nova Player, Just Player, MPV, etc.) while downloading progresses, or play finished videos anytime.
- **Selective File Downloads**: Choose exactly which files, episodes, or bonus materials to download from multi-file torrents.
- **Battery-Friendly & Low-RAM Stability**:
  - Constrained **V8 memory cap (96MB)** to prevent Out-Of-Memory crashes on low-spec devices.
  - Adaptive polling back-off (up to 8s idle interval) to conserve battery during background downloads.
- **Modern Material 3 Design**: Built with Jetpack Compose featuring dynamic colors, light and dark themes, interactive piece bitfield visualizer, and animated video preview frames.
- **Full Swarm Connectivity**: Connects to seeds and peers via DHT, UDP trackers (OpenTrackr and more), and WebTorrent WebSocket trackers, transferring data over TCP, native uTP, and WebRTC data channels.

---

## Installation

Download the latest APK release from [GitHub Releases](https://github.com/karmugilen/torrent-player/releases).

Install via ADB or your device's package installer:
```bash
adb install -r torrent-player.apk
```

---

## Using the App

1. **Add a Torrent**: Paste a magnet link or open a `.torrent` file. The app inspects the metadata and displays the file selection sheet.
2. **Select Files & Destination**: Choose the specific files to download. Files are stored directly in `Downloads/Webtor`. A fresh install does not require broad "All Files Access" storage permissions.
3. **Download & Play**:
   - Tap **Download** to start fetching pieces.
   - Tap **Play** on any video item to open Android's native app picker and stream in-progress or completed files immediately in your chosen media player.
4. **Library & Persistence**: Downloads persist in your library across application restarts. Previously saved metadata-only entries remain safely paused until you explicitly choose Download.
5. **Background Operation & Controls**:
   - **Pause** stops swarm peer connections while keeping downloaded files intact.
   - **Delete** provides the option to remove the entry while either keeping or erasing downloaded media from disk.
   - Ongoing background notifications keep active transfers alive and feature a single-tap **Stop** action to pause downloads conveniently from the notification shade.

---

## Architecture & Swarm Engine

```
┌─────────────────────────────────────────────────────────────┐
│                 Android Application Layer                   │
│      Jetpack Compose (Material 3) • Foreground Service      │
│            LibrarySession • Storage Management              │
└──────────────────────────────┬──────────────────────────────┘
                               │ Loopback IPC (127.0.0.1)
┌──────────────────────────────▼──────────────────────────────┐
│                    Swarm Engine Runtime                     │
│      nodejs-mobile (Node 18.20.4) • WebTorrent 2.2.1        │
│       Native uTP & WebRTC Channels • V8 Memory Cap (96MB)   │
└─────────────────────────────────────────────────────────────┘
```

- **Android Host (`android/app/`, `android/core/`)**:
  - Written in Kotlin with Jetpack Compose.
  - Manages Android foreground services, notifications, intent dispatch to external media players, and library state persistence.
- **Engine Runtime (`engine/`)**:
  - Embedded `nodejs-mobile` (Node 18.20.4) runtime executing `webtorrent@2.2.1`.
  - Built-in native C++ addons for uTP protocol support and WebRTC data channels.
  - Communicates with the Android host through a secure, local-only HTTP/JSON bridge on `127.0.0.1`.

---

## Building from Source

### Prerequisites
- Android SDK & NDK (configured in `android/SDK.env`)
- Node.js 18+ and `npm`
- Java 17+

### Build Steps

```bash
# 1. Install engine dependencies
cd engine && npm ci --omit=optional
cd ..

# 2. Source Android SDK environment and build native node addons
source android/SDK.env
./scripts/vendor-node.sh
./scripts/build-native-addons.sh

# 3. Assemble and install the release APK
cd android && ./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```

> **Note:** `assembleRelease` produces an optimized APK minified with R8 resource shrinking. For iterative development, debug builds can be created using `./gradlew :app:assembleDebug`.

---

## Disclaimer & Legal

Use this application only with torrents and media content that you have the legal right to download or distribute. The test fixtures utilized during development and automated verification are open-source Creative Commons videos (*Big Buck Bunny* and *Sintel* &copy; Blender Foundation, licensed under [CC-BY](https://creativecommons.org/licenses/by/3.0/)).

---

## License

This project is licensed under the [MIT License](LICENSE).

# Torrent Player

<p align="center">
  <img src="docs/images/icon.png" width="80" height="80" alt="Torrent Player Icon" />
  <h2 align="center">Torrent Player</h2>
  <p align="center">
    <strong>Fast, native, battery-friendly torrent downloader and streaming player for Android.</strong>
  </p>
  <p align="center">
    <a href="https://developer.android.com"><img src="https://img.shields.io/badge/Android-8.0%2B%20(API%2026%2B)-3DDC84?style=flat-square&logo=android&logoColor=white" alt="Android 8.0+ (API 26+)"></a>
    <a href="https://m3.material.io"><img src="https://img.shields.io/badge/Material%20Design-3-795548?style=flat-square&logo=materialdesign&logoColor=white" alt="Material Design 3"></a>
    <a href="https://github.com/anacrolix/torrent"><img src="https://img.shields.io/badge/Engine-Go-00ADD8?style=flat-square&logo=go&logoColor=white" alt="Native Go torrent engine"></a>
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
  - Native Go engine with bounded preparation and piece-boundary storage.
  - Native change notifications update the UI without repeated control HTTP requests. Transfer telemetry uses the engine's existing one-second sampling clock.
- **Modern Material 3 Design**: Built with Jetpack Compose featuring dynamic colors, light and dark themes, interactive piece bitfield visualizer, and animated video preview frames.
- **Cached Public Trackers**: A small maintained tracker list refreshes daily in the background. The last good cache works offline, supplied trackers stay intact, and extra trackers are added only after public metadata is available. WebSocket signaling trackers remain enabled. See [tracker discovery details](docs/RELEASE_1.4.4.md).
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
                               │ Direct JNI commands and events
┌──────────────────────────────▼──────────────────────────────┐
│                    Swarm Engine Runtime                     │
│        Native Go engine • anacrolix/torrent                 │
│            TCP • uTP • WebRTC • DHT • PEX                  │
└─────────────────────────────────────────────────────────────┘
```

- **Android Host (`android/app/`, `android/core/`)**:
  - Written in Kotlin with Jetpack Compose.
  - Manages Android foreground services, notifications, intent dispatch to external media players, and library state persistence.
- **Engine Runtime (`engine-go/`)**:
  - Native Go shared library based on `anacrolix/torrent`.
  - Supports TCP, uTP, WebRTC/WebTorrent-compatible peers, DHT, PEX, and HTTP/WebSocket trackers.
  - Android commands run through JNI; there is no Android control listener on port 18080 and no OkHttp dependency.
  - External players receive a read-only Android content URI. Incomplete files use a seekable proxy descriptor backed by verified Go torrent reads; completed files use their saved descriptor.
  - A loopback HTTP media endpoint remains available for streaming compatibility. The standalone host engine also provides an HTTP control adapter for development.

---

## Building from Source

### Prerequisites
- Linux and Git
- Go 1.24 or newer
- Android SDK platform 34, build tools 34.0.0, and NDK 26.1.10909125
- Java 17 or 21; set `JAVA_HOME` and `ANDROID_HOME` for your machine

### Build Steps

```bash
# Gradle compiles engine-go into libengine.so, then packages the APK.
cd android
./gradlew :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`assembleRelease` produces an optimized, unsigned APK for F-Droid to sign.
Upstream releases use the `WEBTOR_KEYSTORE`, `WEBTOR_STORE_PASSWORD`,
`WEBTOR_KEY_ALIAS` and `WEBTOR_KEY_PASSWORD` environment variables for signing.
The build contains no Node.js runtime, npm packages, V8 engine, or JavaScript bundle.

F-Droid inclusion is tracked in [merge request !48893](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893).
See [the submission notes](docs/FDROID_SUBMISSION.md) for the build recipe and validation status.

---

## Disclaimer & Legal

Use this application only with torrents and media content that you have the legal right to download or distribute. The test fixtures utilized during development and automated verification are open-source Creative Commons videos (*Big Buck Bunny* and *Sintel* &copy; Blender Foundation, licensed under [CC-BY](https://creativecommons.org/licenses/by/3.0/)).

---

## License

This project is licensed under the [MIT License](LICENSE).

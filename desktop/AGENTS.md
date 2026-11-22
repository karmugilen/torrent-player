# Torrent Player desktop (Linux)

The Linux app mirrors the Android app in `../android`: same engine, same behaviour,
same defaults. When in doubt about how something should behave, read the Android
code and do what it does. Android sources: `android/app/src/main/java/webtor/app/`
(LibrarySession.kt is the brain, PlayerActivity.kt the player, SettingsScreen.kt the
settings) and `android/core/src/main/kotlin/webtor/core/EngineClient.kt` (every engine
route and its JSON). Engine routes are implemented in `engine-rs/crates/wt-engine/src/`.
Read `docs/DIRECT_PLAY.md` before code that reads the play buffer.

## Stack

- C++17 and Qt 6 Quick, Material Dark, qmake6 (`desktop/torrentplayer.pro`), flat `src/`.
  Conventions are those of the omarchy-app skill: `~/.claude/skills/omarchy-app/SKILL.md`.
- C++ owns logic (QObject with Q_PROPERTY/Q_INVOKABLE, set as context properties in
  `src/main.cpp`); QML owns layout; every key binding is a `Shortcut` with
  `context: Qt.ApplicationShortcut`. QML files use `pageTitle`, never `title`, on pages.
- Every visible string goes through `qsTr()`/`tr()`; counted strings use `%n`.
- The engine is `engine-host` from `../engine-rs`, run by `src/engine.cpp` and spoken to
  over localhost HTTP (`engine.get/post(path, json, callback(reply, status))`).

## Build and check

```sh
cd desktop
TORRENTPLAYER_SKIP_ENGINE=1 bin/build   # Qt app only, quick
bin/test                                # Qt Test unit tests, offscreen
```

A ready engine binary for trying the app is at
`/home/kar/orca/workspaces/torrent-player/desktop/engine-rs/target/release/engine-host`;
run the app with `TORRENTPLAYER_ENGINE=<that path> build/torrentplayer`. To look at the
UI without a display, load it with `QT_QPA_PLATFORM=offscreen QT_FORCE_STDERR_LOGGING=1`
and grab the window (see the omarchy-app skill, Loop step 4). A legal test torrent is
Sintel: `magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.webtorrent.dev`.

## Rules

- Stay inside the files your task names; add new files freely. Don't touch `android/`
  or `engine-rs/` unless your task says so.
- Add unit tests in `tests/torrentplayer_tests.cpp` for new logic (add new sources to
  `tests/torrentplayer_tests.pro` too). Leave `bin/build` and `bin/test` passing.
- Commit your work on your branch with short imperative messages. Never push.
- End with a short summary of what you built, the files and the public interface
  (C++ properties/invokables, QML components) the next agents can use.

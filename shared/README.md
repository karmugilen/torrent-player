# shared/

`policy.json` holds the defaults and tuning that the Android and desktop apps share, so
both apps behave the same:

- `settings`: every user setting, keyed by its Android SharedPreferences key, with its
  default, its range (`min`/`max`) or choices (`options`, `optionLabels`), whether the
  Android settings screen shows it (`visible`), and `platforms` when only one app has it
  (no `platforms` means both).
- `info`: rows the settings screen shows that are not settings (the incoming port).
- `ramCache`: the memory-per-torrent slider steps and the Auto rule.
- `play`: the flags sent with `POST /play` (one built-in playback mode, no switches).
- `engine`: the hidden engine switches sent with `POST /settings` after the engine starts,
  and the order of those requests.

Each section's `_source` names the Kotlin files the values come from. Keys starting
with `_` are notes; programs ignore them.

## Who reads it

- **Desktop** (`desktop/`): compiled into the binary through `desktop/src/resources.qrc`
  (alias `policy.json`) and read by `desktop/src/policy.cpp`. `Settings` takes its
  defaults and ranges from here and `EngineSettings` sends the `engine` values.
- **Android** (`android/`): the `generatePolicy` task in `android/app/build.gradle.kts`
  turns this file into the Kotlin object `webtor.app.Policy` at build time (same names,
  `_` keys left out), so the values are compile-time constants. The places each `_source`
  names read `Policy` instead of their own literals.

## Changing a value

1. Edit `policy.json` (keep it valid JSON: `python3 -m json.tool shared/policy.json`).
2. Update `android/app/src/test/java/webtor/app/PolicyTest.kt`, which pins the values
   on purpose so a change is never accidental, and run
   `cd android && ./gradlew :app:testDebugUnitTest`.
3. Run the desktop checks, which compare the desktop defaults with this file:
   `cd desktop && TORRENTPLAYER_SKIP_ENGINE=1 bin/build && bin/test`.

A new user setting also needs a property in `desktop/src/settings.h` (and its UI).
Saved user values are never rewritten: a new default reaches only users who never
changed that setting.

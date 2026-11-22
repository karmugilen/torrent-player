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
- **Android** (`android/`): still has the same values in Kotlin, at the places each
  `_source` names. Until it loads this file, a change here must be made there too.

## Changing a value

1. Edit `policy.json` (keep it valid JSON: `python3 -m json.tool shared/policy.json`).
2. Make the same change in the Kotlin file named in the section's `_source`.
3. Run the desktop checks, which compare the desktop defaults with this file:
   `cd desktop && TORRENTPLAYER_SKIP_ENGINE=1 bin/build && bin/test`.

A new user setting also needs a property in `desktop/src/settings.h` (and its UI).
Saved user values are never rewritten: a new default reaches only users who never
changed that setting.

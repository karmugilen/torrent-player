# Desktop sync from Android

You are porting user-visible Android app changes into the Linux desktop app in
this worktree. Read `desktop/AGENTS.md` and follow it. Stay inside desktop app
files (`desktop/`, and the version/release plumbing listed there when needed).
Do not touch `android/` or `engine-rs/` unless a path is required only to read
behaviour.

## What to do

1. Review every commit listed below (and its diff). Engine-only commits need no
   code port — the desktop app rebuilds `engine-host` from `engine-rs`. Port the
   user-visible app changes (Kotlin/Compose behaviour, settings, player, library,
   add flow, strings, defaults) into the matching C++/QML desktop code.
2. Shared settings policy lives in `shared/policy.json` when present; prefer it
   over hard-coding defaults that Android already shares.
3. After porting: `cd desktop && bin/build && bin/test` must exit 0. Use
   `TORRENTPLAYER_SKIP_ENGINE=1 bin/build` for a quick Qt-only check, then a full
   `bin/build` if you changed anything that needs the engine binary in `build/`.
4. Update `desktop/.android-synced` to the Android tip SHA given below (the tip
   of `origin/karmugilen/rust-engine` at sync start).
5. Write a short summary to `desktop/sync-reports/DATE.md` (DATE is given below):
   commits reviewed, what you ported, what you skipped and why, build/test
   result, and anything that needs a human decision.
6. Commit on this branch with a short imperative message (for example
   `Port Android library card pictures to desktop`). Include the marker, the
   report, and your code changes. Never merge. Never push.

## Constraints

- Same behaviour and defaults as Android when in doubt.
- Every visible string through `qsTr()`/`tr()`.
- Do not change release version numbers here; `desktop/bin/build` reads Android
  `versionName` into `build/app_version.h`.
- If a change cannot be ported cleanly, note it in the report and leave a TODO
  comment only when it blocks behaviour.

## Android tip and date

- DATE: {{DATE}}
- Android tip (write this into `desktop/.android-synced`): {{ANDROID_TIP}}
- Desktop base branch: {{BASE}}

## Commits that need porting

{{PORT_COMMITS}}

## Engine-only commits (rebuild only; no port)

{{ENGINE_COMMITS}}

## Diffs for commits that need porting

{{PORT_DIFFS}}

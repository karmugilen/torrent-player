# Desktop sync from Android

The Linux app mirrors the Android app: same engine (rebuild), same settings
policy (`shared/policy.json` when present), same version numbers, one release.
User-visible app features are ported by a Claude session (daily and on demand)
onto a review branch; you merge only after approving.

## How sync works

1. Marker file `desktop/.android-synced` holds the last ported commit SHA of
   `origin/karmugilen/rust-engine`.
2. `scripts/desktop-sync.sh` fetches that branch and lists commits after the
   marker. Commits that only touch `artifacts/`, `fastlane/`, `docs/images/`,
   screenshots, store listings, release scripts, or `desktop/` are ignored.
   Commits that only change `engine-rs/` are listed as engine-only (rebuild on
   the next desktop build; no code port).
3. If nothing needs porting, the script exits 0 with no output.
4. Otherwise it creates a fresh git worktree and branch `desktop-sync/<date>`
   off `--base` (default `karmugilen/desktop`), runs headless `claude -p` with
   `desktop/SYNC_PROMPT.md` (commits + diffs filled in), and sends a
   `notify-send` when the session finishes: “Desktop sync ready for review:
   \<branch\>”.
5. The Claude session ports changes, runs `cd desktop && bin/build && bin/test`,
   updates the marker, writes `desktop/sync-reports/<date>.md`, and commits on
   that branch. It never merges and never pushes.

Worktrees live under `~/.cache/torrentplayer-desktop-sync/<date>`.

## Review and merge a sync branch

1. Open the notification branch (or `git worktree list` / the path printed by
   the script).
2. Read `desktop/sync-reports/<date>.md` and the commit(s) on `desktop-sync/<date>`.
3. Build and try the app if you want:
   `cd desktop && TORRENTPLAYER_SKIP_ENGINE=1 bin/build && bin/test`.
4. When you approve, merge into your desktop branch (for example
   `karmugilen/desktop`) in your usual way. Do not push unless you mean to.
5. Remove the sync worktree when finished:
   `git worktree remove ~/.cache/torrentplayer-desktop-sync/<date>`.

## On demand

From the repo root:

```sh
scripts/desktop-sync.sh --dry-run   # list what would be ported
scripts/desktop-sync.sh             # create branch + run Claude
scripts/desktop-sync.sh --base karmugilen/desktop
```

Or in Claude Code, run the slash command `/sync-desktop` (see
`.claude/commands/sync-desktop.md`): dry-run first, then the real sync if there
is something to port.

## Daily timer

User systemd units live in `desktop/sync/`:

- `torrentplayer-sync.service` — oneshot, runs `scripts/desktop-sync.sh`
- `torrentplayer-sync.timer` — daily, `Persistent=true`

Install once (copies into `~/.config/systemd/user` and enables the timer):

```sh
scripts/desktop-sync-install.sh
```

Check with `systemctl --user list-timers torrentplayer-sync.timer`.

## Version and release

Desktop `versionName` is Android’s `versionName` in
`android/app/build.gradle.kts`:

- `desktop/bin/build` writes `build/app_version.h` (`APP_VERSION`) for the app
- `desktop/pkgbuild/PKGBUILD` `pkgver` reads the same field
- `torrentplayer --version` prints it (`desktop/src/main.cpp`)

`scripts/ship.sh` still ships Android first. After the GitHub release exists, an
optional step builds the Arch package with `makepkg` (no install) and uploads
`torrentplayer-<version>-1-x86_64.pkg.tar.zst` to the same release. The step is
skipped when `qmake6` is missing, and a failure there never fails the Android
release.

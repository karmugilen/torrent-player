# Torrent Player (webtor.app)

- **One session, one worktree.** Work on your own branch in your own worktree (Orca makes one). Before committing, run `git worktree list`: if another worktree has your branch checked out, stop and ask the user. To undo committed work, use `git revert`, never reset.
- **Release:** `scripts/ship.sh <x.y.z>` does everything (see `docs/PLAY_STORE.md`, Each later release).
- **Phone build:** `scripts/phone-build.sh <name>` (release key, saved in `artifacts/`, refuses while the player is open). Reinstall an old build with `scripts/install-apk.sh <name>`.
- **GitHub has one branch, `karmugilen/rust-engine`** (F-Droid builds from `v*` tags). Push there: `git push origin HEAD:karmugilen/rust-engine`; make no other branches on GitHub.
- **Before a build, push or release:** read the webtor section of `~/jornalAndLog/MISTAKES.md`. Releases go out weekly on Sundays only.
- **Engine:** Rust, in `engine-rs/` (Android build: `engine-rs/docs/ANDROID_BUILD.md`). Read `docs/DIRECT_PLAY.md` before code that reads the play buffer: most buffered bytes are blocks received but not yet verified.
- **Player logs:** logcat tags `webtor-player` (app) and `webtor-mpv` (mpv warnings and errors).
- **Checks:** `.githooks/pre-commit` compiles the side you staged (Kotlin and/or Rust); enable it once per clone with `git config core.hooksPath .githooks`. Tests are not in the hook yet: 4 fail on this code (`RamCachePolicyTest` x2, `DownloadEntryTest`, Rust `test_cache_shrink_preserves_dirty_download_bytes`).

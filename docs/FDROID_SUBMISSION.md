# F-Droid releases: Torrent Player (`webtor.app`)

Torrent Player is published on F-Droid: <https://f-droid.org/packages/webtor.app/>.
The inclusion merge request [fdroiddata !48893](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893)
was merged on 2026-10-17 with 2.0.0 (23). The live recipe is
[`metadata/webtor.app.yml`](https://gitlab.com/fdroid/fdroiddata/-/blob/master/metadata/webtor.app.yml);
[`webtor.app.yml`](../webtor.app.yml) here is a reference copy.

## How updates reach F-Droid

The recipe uses `UpdateCheckMode: Tags` and `AutoUpdateMode: Version`. F-Droid's
checkupdates bot finds a new `v*` tag, reads `versionName` and `versionCode`
from `android/app/build.gradle.kts` at that tag and adds a build that copies the
previous one. Nothing needs to be submitted by hand as long as the build steps
stay the same. Since 2.4.0 (100) they are: Debian `gcc` for Rust build scripts, the `rustup@1.29.2` srclib installs
Rust 1.99.1 with the `aarch64-linux-android` target, NDK r26b, then Gradle in
`android/app`, which runs `engine-rs/build-android.sh` (`cargo build --locked`).
Up to 2.3.0 (99) the engine was Go (Debian `golang-go`, `go mod download`).

When the build steps change, add the new version's block to
[`webtor.app.yml`](../webtor.app.yml) here; `scripts/fdroid-release.sh` builds
with that block, and the same block goes into an fdroiddata merge request.

The builds are reproducible. `Binaries:` points at
`https://github.com/karmugilen/torrent-player/releases/download/v%v/torrent-player-%v.apk`
and `AllowedAPKSigningKeys` is the release key
(`b26e9a22c1a40c5f8bd2bb5ba9e8639f7835c1ca986e9607e240beeddbd9f71f`). F-Droid
rebuilds the tag, copies the GitHub APK's signature onto its build and
publishes the GitHub APK only if they match. The GitHub APK must be built in
F-Droid's buildserver image, not with a local Gradle: local builds differ in
`classes.dex`, `baseline.prof` and `libengine.so`.

## Releasing a version

1. Raise `versionCode` and `versionName` in `android/app/build.gradle.kts` and
   add `fastlane/metadata/android/en-US/changelogs/<versionCode>.txt`
   (500 characters or fewer).
2. Commit and push the branch to GitHub. Do not tag yet.
3. Run `scripts/fdroid-release.sh` (defaults to `HEAD`). It clones fdroiddata,
   adds the build as checkupdates would, runs the fdroiddata CI build job in
   `registry.gitlab.com/fdroid/fdroidserver:buildserver-trixie`
   (`scripts/fdroid-ci-build.sh`, including F-Droid's source and APK scans),
   signs the APK with the release key (see [SIGNING.md](SIGNING.md)) and checks
   that the signer is the allowed key. The APK lands in
   `artifacts/torrent-player-<versionName>.apk`.
4. Create the GitHub release with that APK in one step, so the tag never exists
   without the APK:
   `gh release create v<versionName> artifacts/torrent-player-<versionName>.apk --target <commit> ...`
5. Never replace an APK that has been published. Fix problems in a new version.

Sign with `apksigner sign --alignment-preserved` (the script does). Without it,
build-tools 35 and newer re-align the zip while signing, and F-Droid's
verification fails because the signature no longer fits its build. The script
runs F-Droid's own `verify_apks` check before it reports success.

## History

- 2.4.1 (101): .torrent files and links, free-space fix for chosen folders,
  settings cleanup, seek A/B UI removed. Same Rust build steps as 2.4.0;
  F-Droid checkupdates picks up the `v2.4.1` tag.
- 2.4.0 (100): the engine moved from Go to Rust (`engine-rs/`, MPL-2.0 because
  it derives from anacrolix/torrent). Live fdroiddata uses Debian `rustup`
  with toolchain 1.99.1 and `aarch64-linux-android`.
- 2.2.0 (98), 2026-10-25: first update after inclusion. The Android engine stopped
  linking the Rust helpers, so the recipe stayed Go-only, and trackers and peers
  see the real app version. Built and signed with `scripts/fdroid-release.sh`;
  two builds from different commits were byte-identical, F-Droid's
  `verify_apks` accepted the GitHub APK and `fdroid checkupdates` picked up
  `v2.2.0` (98). The first upload, signed without `--alignment-preserved`, was
  replaced minutes after release, before F-Droid fetched it.
- 2.0.0 (23): reviewed in !48893 (reviewer @mezinster, 2026-10-03; VirusTotal 0/67).
  Follow-ups disclosed the tracker-list refresh, extra public trackers and
  STUN providers in the description and added changelog 23. Pipeline
  [2910503829](https://gitlab.com/karmugilrc/fdroiddata/-/pipelines/2910503829)
  passed all nine jobs including the reproducible comparison.
- 1.4.4 (21) and 1.4.5 (22): earlier recipe versions during review.

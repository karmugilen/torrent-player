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
stay the same: Debian `golang-go` (1.24), NDK r26b, `go mod download` and
`go mod verify`, then Gradle in `android/app`. The engine must therefore stay
pure Go (no Rust or other toolchains) and `go.mod` must stay at `go 1.24`.

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

## History

- 2.2.0 (98), 2026-10-25: first update after inclusion. The Android engine stopped
  linking the Rust helpers, so the recipe stayed Go-only, and trackers and peers
  see the real app version. Built and signed with `scripts/fdroid-release.sh`.
- 2.0.0 (23): reviewed in !48893 (reviewer @mezinster, 2026-10-03; VirusTotal 0/67).
  Follow-ups disclosed the tracker-list refresh, extra public trackers and
  STUN providers in the description and added changelog 23. Pipeline
  [2910503829](https://gitlab.com/karmugilrc/fdroiddata/-/pipelines/2910503829)
  passed all nine jobs including the reproducible comparison.
- 1.4.4 (21) and 1.4.5 (22): earlier recipe versions during review.

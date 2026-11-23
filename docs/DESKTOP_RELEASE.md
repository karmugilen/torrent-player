# Desktop release (Linux, Windows, macOS)

GitHub Actions builds the desktop app (`.github/workflows/desktop.yml`) and attaches
it to an existing GitHub release. The workflow only uploads assets: it never creates
tags, releases or branches, and never touches the APK.

## Run it

Normally nothing to do: `scripts/ship.sh` publishes the release, and publishing
starts the workflow (`release: published`) when the tagged commit has it. Step 7b
of `ship.sh` checks for that run and starts one itself if none appears.

By hand, for example to add the desktop apps to a release that already exists:

```sh
gh workflow run desktop.yml -R karmugilen/torrent-player --ref karmugilen/rust-engine -f tag=v2.6.0
gh run watch -R karmugilen/torrent-player "$(gh run list -R karmugilen/torrent-player --workflow desktop.yml -L 1 --json databaseId --jq '.[0].databaseId')"
```

- A manual run builds the branch it was started on (`--ref`), not the tag, so newer
  desktop code can go onto an older release. A release event builds the tag.
- The version is `versionName` in `android/app/build.gradle.kts`. The run stops if
  it doesn't match the tag (`v2.6.0` needs `versionName = "2.6.0"`), or if no release
  has that tag.
- Uploads use `--clobber`, so a rerun replaces the files. To redo one platform, rerun
  that job from the Actions page. The checksums job runs again after it.
- The workflow must be on the default branch (`karmugilen/rust-engine`) before
  `gh workflow run` can find it.

## What it attaches (V = versionName)

| File | Built on | Notes |
|---|---|---|
| `TorrentPlayer-V-linux-x86_64.AppImage` | ubuntu-22.04, Qt 6.8 (aqt), libmpv from Ubuntu | linuxdeploy + plugin-qt; runs on glibc 2.35+ |
| `torrentplayer-V-1-x86_64.pkg.tar.zst` | `archlinux:base-devel` container | `desktop/pkgbuild/PKGBUILD`, `pacman -U` it |
| `TorrentPlayer-V-windows-x64-setup.exe` | windows-2022, MSVC, Qt 6.8 | Inno Setup, per-user, no admin; `/VERYSILENT /SUPPRESSMSGBOXES` |
| `TorrentPlayer-V-windows-x64-portable.zip` | same | unzip and run `TorrentPlayer\torrentplayer.exe` |
| `TorrentPlayer-V-macos-arm64.dmg` | macos-14 | Apple silicon |
| `TorrentPlayer-V-macos-x64.dmg` | macos-15-intel | Intel Macs |
| `SHA256SUMS-desktop.txt` | | `sha256sum -c SHA256SUMS-desktop.txt --ignore-missing` |

libmpv comes from Ubuntu (AppImage), Arch (package), shinchiro/mpv-winbuild-cmake
(Windows, the newest `mpv-dev-x86_64-*.7z`) and Homebrew (macOS). The macOS builds
need at least the macOS version of the runner that built them (Homebrew bottles
target it): macOS 14 on Apple silicon, macOS 15 on Intel. GitHub retired its
macos-13 image in December 2025, so macos-15-intel is the oldest Intel runner left.

The packaging scripts the workflow runs are in `desktop/packaging/`:
`linux/appimage.sh` (also works locally: `desktop/packaging/linux/appimage.sh 2.6.0 dist`
after `desktop/bin/build`), `windows/build.ps1` + `windows/torrentplayer.iss`, and
`macos/package.sh`.

## First run of the unsigned apps

The apps are not signed with a paid certificate, so each OS warns once.

- **Windows**: SmartScreen says "Windows protected your PC". Click **More info**,
  then **Run anyway**.
- **macOS**: open the dmg and drag Torrent Player to Applications. The first time,
  right-click (or Control-click) the app, choose **Open**, then **Open** again. On
  macOS 15 and later, if there is no Open button: System Settings > Privacy &
  Security > **Open Anyway**. Or, in Terminal:
  `xattr -dr com.apple.quarantine "/Applications/Torrent Player.app"`.
- **Linux AppImage**: `chmod +x TorrentPlayer-*.AppImage`, then run it. Without
  FUSE 2 (`libfuse2`), run it with `--appimage-extract-and-run`.

## Why F-Droid and Google Play are unaffected

- **One tag per release.** Releases still have exactly one `vX.Y.Z` tag, made by
  `scripts/release.sh`. F-Droid builds from that tag. The workflow makes no tags.
- **The APK is attached first.** `release.sh` creates the release with the APK.
  Desktop files are added afterwards, under their own names. The workflow never
  deletes, replaces or downloads the APK, so the APK and its SHA-256 in the notes
  don't change.
- **No binaries in git.** Everything is built in CI and uploaded to the release.
  Nothing compiled is committed, so F-Droid's scan of the repo is unchanged. The new
  files are text: the workflow, scripts, the Inno Setup script and this doc.
- **Play** uses the AAB from `ship.sh`. The workflow doesn't touch it.
- **Android isn't held up by desktop failures.** `ship.sh` step 7b only starts the
  workflow, and a failed desktop build leaves the Android release as it is.

## Only CI can confirm

These parts were checked with actionlint and shellcheck and dry-read, but only run
on GitHub's runners: the Windows build (MSVC, mpv.lib from the `.def`, windeployqt,
Inno Setup), the macOS bundle (macdeployqt, dylibbundler, ad-hoc signing, hdiutil),
the aqt Qt 6.8 install on each OS, and the Arch container job. The AppImage script
has been run locally on Arch.

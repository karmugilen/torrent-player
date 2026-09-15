# F-Droid submission: Torrent Player (`webtor.app`)

The active submission is [fdroiddata merge request !48893](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893). Update that branch instead of opening a duplicate merge request.

## Build recipe

The canonical recipe is [`webtor.app.yml`](../webtor.app.yml). The app now builds a native Go engine from `engine-go/` with Android NDK r26b. It does not use Node.js, npm, V8, CMake, JavaScript bundles, or native Node addons.

Gradle runs `engine-go/build.sh` before packaging. The build requires Go 1.24 or newer, Java 17 or 21, Android SDK 34, and NDK 26.1.10909125. The Android release APK contains `libengine.so` for `arm64-v8a`.

Before submitting a release, update the recipe's full commit SHA and version fields, then validate from a clean checkout in F-Droid's current buildserver image:

```sh
fdroid rewritemeta webtor.app
fdroid lint webtor.app
fdroid checkupdates --auto webtor.app
fdroid build --latest webtor.app
```

The recipe installs Debian Go 1.24.4, downloads the exact go.mod/go.sum dependencies during prebuild, and runs `go mod verify`. `GOTOOLCHAIN=local` prevents automatic toolchain downloads. The local anet source patch replaces its unused precompiled mobile binaries. Packaged Go dependency notices can be refreshed with `python3 scripts/update-go-notices.py` after building libengine.so. Do not claim an official successful build until the fdroiddata pipeline has built and scanned the new Go-engine release.

## Submit updates

After testing and pushing the referenced upstream release commit, run:

```sh
./scripts/submit-to-fdroid.sh
```

The script updates only the existing submission branch. F-Droid maintainers control review, builds, signing, and publication.

## Release 1.4.4 validation (2026-09-15)

- Release source: `cb711809578404b46761ecb679591c34b34d9cdb`, version 1.4.4 (21).
- F-Droid 2.4.2 in `buildserver-trixie`, Debian Go 1.24.4 and NDK r26b:
  metadata lint/rewritemeta, module verification, clean source scan, release
  build, APK identity checks and APK binary scan all passed.
- Command: `fdroid build -v --latest --test --scan-binary webtor.app`.
- The first empty local workspace exposed F-Droid 2.4.2's SOURCE_DATE_EPOCH
  lookup-before-clone error. Pre-cloning the repository resolved that tooling
  issue; subsequent builds used F-Droid's normal checkout/clean/scanning flow.
- The new recipe removes the previous Node/npm/srclib preparation and retains
  full commit pinning. No binary/source scanner exemptions were added.
- F-Droid review and publication remain controlled by maintainers. Check the
  existing MR for the current upstream pipeline and review status.

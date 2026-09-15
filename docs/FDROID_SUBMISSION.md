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

#!/usr/bin/env bash
# Build a release-key APK, save it in artifacts/, install it on the phone (data kept).
#   scripts/phone-build.sh <name>            engine + app, then install
#   scripts/phone-build.sh <name> --app      app only (Kotlin change; reuse the built engine)
#   scripts/phone-build.sh <name> --no-install
# The phone holds the release-key build, so local-key APKs (fast-build.sh) are refused there.
# Refuses to install while the player is open on the phone; set FORCE=1 to install anyway.
set -euo pipefail

name="${1:?usage: scripts/phone-build.sh <name> [--app] [--no-install]}"
shift
engine=1 install=1
for arg in "$@"; do
    case "$arg" in
        --app) engine=0 ;;
        --no-install) install=0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
export PATH="$HOME/.cargo/bin:$PATH"
unset RUSTC_WRAPPER # sccache keeps the Arch toolchain (E0463); see MISTAKES.md

creds="$HOME/.config/webtor/credentials.env"
[ -f "$creds" ] || { echo "missing $creds (release key)" >&2; exit 1; }
set -a
# shellcheck disable=SC1090
. "$creds" >/dev/null 2>&1
set +a

if [ "$engine" -eq 1 ]; then
    "$ROOT/engine-rs/build-android.sh"
fi
(cd "$ROOT/android" && ./gradlew :app:assembleRelease -x :app:buildRustEngine --no-configuration-cache -q)

version="$(grep -m1 -oP 'versionName = "\K[^"]+' "$ROOT/android/app/build.gradle.kts")"
apk="$ROOT/artifacts/torrent-player-$version-$name-releasekey.apk"
mkdir -p "$ROOT/artifacts"
cp "$ROOT/android/app/build/outputs/apk/release/app-release.apk" "$apk"
(cd "$ROOT/artifacts" && sha256sum "${apk##*/}" > "${apk##*/}.sha256")
echo "Built ${apk##*/}"

[ "$install" -eq 1 ] || exit 0

echo "Waiting for the phone on adb..."
timeout 30 adb wait-for-device || { echo "No phone on adb. Install later: scripts/install-apk.sh $name" >&2; exit 1; }
top="$(timeout 10 adb shell dumpsys activity activities | grep -m1 topResumedActivity || true)"
if [[ "$top" == *webtor.app/.PlayerActivity* && "${FORCE:-0}" != 1 ]]; then
    echo "The player is open on the phone; installing would close it. Re-run with FORCE=1 or: scripts/install-apk.sh $name" >&2
    exit 3
fi
timeout 180 adb install -r "$apk"
echo "Installed ${apk##*/}"

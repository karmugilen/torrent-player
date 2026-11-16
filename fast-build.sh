#!/usr/bin/env bash
# CloudflareFastBuild: local targeted assemble + install (sideload only).
# GitHub / F-Droid releases MUST use scripts/fdroid-release.sh instead.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
MODE="${1:-debug}"
SERIAL="${ADB_SERIAL:-00121649A004784}"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/26.1.10909125}"
export PATH="$HOME/.cargo/bin:$PATH"

case "$MODE" in
  debug)
    "$ROOT/engine-rs/build-android.sh"
    "$ROOT/android/gradlew" -p "$ROOT/android" :app:assembleDebug \
      --daemon -Dorg.gradle.parallel=true -Dorg.gradle.caching=true
    APK="$ROOT/android/app/build/outputs/apk/debug/app-debug.apk"
    ;;
  release)
    "$ROOT/engine-rs/build-android.sh"
    "$ROOT/android/gradlew" -p "$ROOT/android" :app:assembleRelease \
      -Pwebtor.localSigning=true \
      --daemon -Dorg.gradle.parallel=true -Dorg.gradle.caching=true
    APK="$ROOT/android/app/build/outputs/apk/release/app-release.apk"
    ;;
  *)
    echo "usage: $0 debug|release" >&2
    exit 1
    ;;
esac

adb -s "$SERIAL" install -r "$APK"
adb -s "$SERIAL" shell am start -n webtor.app/.MainActivity
echo "Installed $APK (localSigning sideload — not for GitHub/F-Droid)"

#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ZIP="$ROOT/vendor/nodejs-mobile/nodejs-mobile-v18.20.4-android.zip"
UNPACK="$ROOT/vendor/nodejs-mobile/unpacked"
JNI="$ROOT/android/app/src/main/jniLibs/arm64-v8a"
INC="$ROOT/android/app/src/main/cpp/include"

if [ ! -f "$ZIP" ]; then
  mkdir -p "$(dirname "$ZIP")"
  echo "Fetching nodejs-mobile v18.20.4 android archive..."
  curl -fSL "https://github.com/nodejs-mobile/nodejs-mobile/releases/download/v18.20.4/nodejs-mobile-v18.20.4-android.zip" -o "$ZIP"
fi

rm -rf "$UNPACK"
mkdir -p "$UNPACK" "$JNI" "$INC"
unzip -qo "$ZIP" -d "$UNPACK"

SO="$(find "$UNPACK" -path '*arm64-v8a*libnode.so' | head -n1)"
if [ -z "$SO" ]; then
  SO="$(find "$UNPACK" -name 'libnode.so' | head -n1)"
fi
[ -n "$SO" ] || { echo "libnode.so not in zip" >&2; find "$UNPACK" | head; exit 1; }
cp "$SO" "$JNI/libnode.so"
if command -v llvm-strip >/dev/null 2>&1; then
  llvm-strip --strip-unneeded "$JNI/libnode.so" || true
elif [ -n "${ANDROID_NDK_HOME:-}${ANDROID_HOME:-}" ]; then
  STRIP="$(find "${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk}" -name llvm-strip -type f 2>/dev/null | head -n1)"
  [ -n "$STRIP" ] && "$STRIP" --strip-unneeded "$JNI/libnode.so" || true
fi

HDR="$(find "$UNPACK" -name 'node.h' | head -n1)"
[ -n "$HDR" ] || { echo "node.h not in zip" >&2; exit 1; }
HDR_DIR="$(dirname "$HDR")"
# copy the include tree that contains node.h
cp -a "$HDR_DIR"/. "$INC"/
echo "libnode: $JNI/libnode.so ($(stat -c%s "$JNI/libnode.so") bytes)"
echo "headers: $INC (from $HDR_DIR)"

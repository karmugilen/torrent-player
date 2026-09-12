#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
: "${ANDROID_HOME:?Source android/SDK.env first}"
NDK="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/26.1.10909125}"
CMAKE="$ANDROID_HOME/cmake/3.22.1/bin/cmake"
BUILD="$ROOT/android/build/native-addons"
OUT="$ROOT/android/app/src/main/jniLibs/arm64-v8a"
if [[ "${FORCE_NATIVE_ADDONS:-}" != "1" && -f "$OUT/libutp_native.so" && -f "$OUT/libnode_datachannel.so" ]]; then
  echo "native addons already in $OUT"
  exit 0
fi
"$CMAKE" -S "$ROOT/scripts/native-addons" -B "$BUILD" -G Ninja \
  -DCMAKE_MAKE_PROGRAM="$ANDROID_HOME/cmake/3.22.1/bin/ninja" \
  -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-26 \
  -DANDROID_STL=c++_shared -DCMAKE_BUILD_TYPE=Release
"$CMAKE" --build "$BUILD" --target utp_native node_datachannel --parallel 4
for name in utp_native node_datachannel; do
  cp "$BUILD/lib$name.so" "$ROOT/android/app/src/main/jniLibs/arm64-v8a/lib$name.so"
  "$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" --strip-unneeded \
    "$ROOT/android/app/src/main/jniLibs/arm64-v8a/lib$name.so"
done

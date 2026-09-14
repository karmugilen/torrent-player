#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
: "${ANDROID_HOME:?Set ANDROID_HOME to your Android SDK}"
NDK="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/26.1.10909125}"
CMAKE="$ANDROID_HOME/cmake/3.22.1/bin/cmake"
BUILD="$ROOT/android/build/native-addons"
OUT="$ROOT/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$OUT"
"$CMAKE" -S "$ROOT/scripts/native-addons" -B "$BUILD" -G Ninja \
  -DCMAKE_MAKE_PROGRAM="$ANDROID_HOME/cmake/3.22.1/bin/ninja" \
  -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-26 \
  -DANDROID_STL=c++_shared -DCMAKE_BUILD_TYPE=Release
"$CMAKE" --build "$BUILD" --target utp_native node_datachannel --parallel "${WEBTOR_BUILD_JOBS:-2}"
for name in utp_native node_datachannel; do
  cp "$BUILD/lib$name.so" "$ROOT/android/app/src/main/jniLibs/arm64-v8a/lib$name.so"
  "$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip" --strip-unneeded \
    "$ROOT/android/app/src/main/jniLibs/arm64-v8a/lib$name.so"
done
LICENSES="$ROOT/android/app/src/main/assets/licenses"
mkdir -p "$LICENSES"
cp "$ROOT/vendor/libdatachannel/LICENSE" "$LICENSES/libdatachannel.txt"
cp "$ROOT/vendor/libdatachannel/deps/libjuice/LICENSE" "$LICENSES/libjuice.txt"
cp "$ROOT/vendor/libdatachannel/deps/usrsctp/LICENSE.md" "$LICENSES/usrsctp.txt"
cp "$ROOT/vendor/libdatachannel/deps/plog/LICENSE" "$LICENSES/plog.txt"

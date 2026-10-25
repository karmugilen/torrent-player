#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${ANDROID_HOME:-}/ndk/26.1.10909125}}"
if [ ! -d "$NDK" ]; then
    echo "Android NDK 26.1.10909125 not found; set ANDROID_NDK_HOME" >&2
    exit 1
fi
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
# Phones use arm64-v8a. ENGINE_ABI=x86_64 builds for the x86_64 emulator.
ABI="${ENGINE_ABI:-arm64-v8a}"
case "$ABI" in
    arm64-v8a) CLANG=aarch64-linux-android26; GOARCH_ABI=arm64 ;;
    x86_64) CLANG=x86_64-linux-android26; GOARCH_ABI=amd64 ;;
    *) echo "Unsupported ENGINE_ABI $ABI" >&2; exit 1 ;;
esac
CC="$TOOLCHAIN/bin/$CLANG-clang"
CXX="$TOOLCHAIN/bin/$CLANG-clang++"
OUT="$ROOT/android/app/src/main/jniLibs/$ABI"

mkdir -p "$OUT"

# Pure Go, so F-Droid needs only Go and the NDK (engine-rust is not linked).
VERSION="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' "$ROOT/android/app/build.gradle.kts")"
echo "Building Go engine for Android $ABI..."
cd "$ROOT/engine-go"

GOTOOLCHAIN=local \
CGO_ENABLED=1 \
GOOS=android \
GOARCH="$GOARCH_ABI" \
CC="$CC" \
CXX="$CXX" \
CGO_LDFLAGS="-static-libstdc++" \
go build -tags=nosqlite,noboltdb -trimpath -buildvcs=false -buildmode=c-shared -ldflags="-s -w -buildid= -X main.appVersion=$VERSION" -o "$OUT/libengine.so" .

if [ -f "$TOOLCHAIN/bin/llvm-strip" ]; then
    "$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$OUT/libengine.so"
fi

echo "Successfully built $OUT/libengine.so ($(stat -c%s "$OUT/libengine.so") bytes)"

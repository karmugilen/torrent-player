#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${ANDROID_HOME:-}/ndk/26.1.10909125}}"
if [ ! -d "$NDK" ]; then
    echo "Android NDK 26.1.10909125 not found; set ANDROID_NDK_HOME" >&2
    exit 1
fi
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
CC="$TOOLCHAIN/bin/aarch64-linux-android26-clang"
CXX="$TOOLCHAIN/bin/aarch64-linux-android26-clang++"
OUT="$ROOT/android/app/src/main/jniLibs/arm64-v8a"

mkdir -p "$OUT"

echo "Building Go engine for Android arm64-v8a..."
cd "$ROOT/engine-go"

GOTOOLCHAIN=local \
CGO_ENABLED=1 \
GOOS=android \
GOARCH=arm64 \
CC="$CC" \
CXX="$CXX" \
CGO_LDFLAGS="-static-libstdc++" \
go build -tags=nosqlite,noboltdb -trimpath -buildvcs=false -buildmode=c-shared -ldflags="-s -w -buildid=" -o "$OUT/libengine.so" .

if [ -f "$TOOLCHAIN/bin/llvm-strip" ]; then
    "$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$OUT/libengine.so"
fi

echo "Successfully built $OUT/libengine.so ($(stat -c%s "$OUT/libengine.so") bytes)"

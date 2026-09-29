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
AR="$TOOLCHAIN/bin/llvm-ar"
OUT="$ROOT/android/app/src/main/jniLibs/arm64-v8a"
RUST_TARGET="aarch64-linux-android"
RUST_OUT="$ROOT/engine-rust/target/$RUST_TARGET/release"

# Prefer rustup toolchains when present so the Android std target is available.
export PATH="${HOME}/.cargo/bin:${PATH}"

mkdir -p "$OUT"

echo "Building Rust piece aggregator for Android arm64-v8a..."
cd "$ROOT/engine-rust"
CC_aarch64_linux_android="$CC" \
AR_aarch64_linux_android="$AR" \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC" \
cargo build --release --target "$RUST_TARGET"
if [ ! -f "$RUST_OUT/libwebtor_pieces.a" ]; then
    echo "Missing $RUST_OUT/libwebtor_pieces.a" >&2
    exit 1
fi

echo "Building Go engine for Android arm64-v8a..."
cd "$ROOT/engine-go"

GOTOOLCHAIN=local \
CGO_ENABLED=1 \
GOOS=android \
GOARCH=arm64 \
CC="$CC" \
CXX="$CXX" \
CGO_LDFLAGS="-static-libstdc++" \
go build -tags=nosqlite,noboltdb,rust -trimpath -buildvcs=false -buildmode=c-shared -ldflags="-s -w -buildid=" -o "$OUT/libengine.so" .

if [ -f "$TOOLCHAIN/bin/llvm-strip" ]; then
    "$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$OUT/libengine.so"
fi

echo "Successfully built $OUT/libengine.so ($(stat -c%s "$OUT/libengine.so") bytes)"

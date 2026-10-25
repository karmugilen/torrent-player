#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${ANDROID_HOME:-}/ndk/26.1.10909125}}"
if [ ! -d "$NDK" ]; then
    echo "Android NDK 26.1.10909125 not found; set ANDROID_NDK_HOME" >&2
    exit 1
fi
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
AR="$TOOLCHAIN/bin/llvm-ar"
# Phones use arm64-v8a. ENGINE_ABI=x86_64 builds for the x86_64 emulator.
ABI="${ENGINE_ABI:-arm64-v8a}"
case "$ABI" in
    arm64-v8a) CLANG=aarch64-linux-android26; GOARCH_ABI=arm64; RUST_TARGET=aarch64-linux-android ;;
    x86_64) CLANG=x86_64-linux-android26; GOARCH_ABI=amd64; RUST_TARGET=x86_64-linux-android ;;
    *) echo "Unsupported ENGINE_ABI $ABI" >&2; exit 1 ;;
esac
CC="$TOOLCHAIN/bin/$CLANG-clang"
CXX="$TOOLCHAIN/bin/$CLANG-clang++"
OUT="$ROOT/android/app/src/main/jniLibs/$ABI"
RUST_OUT="$ROOT/engine-rust/target/$RUST_TARGET/release"
RUST_VAR="${RUST_TARGET//-/_}"
RUST_LINKER_VAR="CARGO_TARGET_$(echo "$RUST_VAR" | tr '[:lower:]' '[:upper:]')_LINKER"

# Prefer rustup toolchains when present so the Android std target is available.
export PATH="${HOME}/.cargo/bin:${PATH}"

mkdir -p "$OUT"

echo "Building Rust piece aggregator for Android $ABI..."
cd "$ROOT/engine-rust"
env "CC_$RUST_VAR=$CC" "AR_$RUST_VAR=$AR" "$RUST_LINKER_VAR=$CC" \
    cargo build --release --target "$RUST_TARGET"
if [ ! -f "$RUST_OUT/libwebtor_pieces.a" ]; then
    echo "Missing $RUST_OUT/libwebtor_pieces.a" >&2
    exit 1
fi

echo "Building Go engine for Android $ABI..."
cd "$ROOT/engine-go"

GOTOOLCHAIN=local \
CGO_ENABLED=1 \
GOOS=android \
GOARCH="$GOARCH_ABI" \
CC="$CC" \
CXX="$CXX" \
CGO_LDFLAGS="-static-libstdc++" \
go build -tags=nosqlite,noboltdb,rust -trimpath -buildvcs=false -buildmode=c-shared -ldflags="-s -w -buildid=" -o "$OUT/libengine.so" .

if [ -f "$TOOLCHAIN/bin/llvm-strip" ]; then
    "$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$OUT/libengine.so"
fi

echo "Successfully built $OUT/libengine.so ($(stat -c%s "$OUT/libengine.so") bytes)"

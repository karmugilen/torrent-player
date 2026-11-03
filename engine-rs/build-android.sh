#!/usr/bin/env bash
# Build wt-engine as libengine.so for Android.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENGINE_RS="$(cd "$(dirname "$0")" && pwd)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${ANDROID_HOME:-}/ndk/26.1.10909125}}"
if [ ! -d "$NDK" ]; then
    echo "Android NDK 26.1.10909125 not found; set ANDROID_NDK_HOME" >&2
    exit 1
fi

TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
# Phones use arm64-v8a. ENGINE_ABI=x86_64 builds for the x86_64 emulator.
ABI="${ENGINE_ABI:-arm64-v8a}"
case "$ABI" in
    arm64-v8a)
        CLANG_TRIPLE=aarch64-linux-android26
        RUST_TARGET=aarch64-linux-android
        CARGO_TARGET_PREFIX=CARGO_TARGET_AARCH64_LINUX_ANDROID
        ;;
    x86_64)
        CLANG_TRIPLE=x86_64-linux-android26
        RUST_TARGET=x86_64-linux-android
        CARGO_TARGET_PREFIX=CARGO_TARGET_X86_64_LINUX_ANDROID
        ;;
    *)
        echo "Unsupported ENGINE_ABI $ABI" >&2
        exit 1
        ;;
esac

CC="$TOOLCHAIN/bin/$CLANG_TRIPLE-clang"
CXX="$TOOLCHAIN/bin/$CLANG_TRIPLE-clang++"
AR="$TOOLCHAIN/bin/llvm-ar"
STRIP_BIN="$TOOLCHAIN/bin/llvm-strip"
OUT="$ROOT/android/app/src/main/jniLibs/$ABI"
OUT_SO="$OUT/libengine.so"
CACHE_ROOT="${WT_CARGO_CACHE_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/wt-targets}"
# Never use /tmp (RAM-backed tmpfs); keep Android builds on disk.
TARGET_DIR="${CARGO_TARGET_DIR:-$CACHE_ROOT/android-$ABI}"
LOCK_FILE="$CACHE_ROOT/android-release.lock"

if [ ! -x "$CC" ]; then
    echo "NDK clang not found at $CC" >&2
    exit 1
fi

mkdir -p "$OUT" "$TARGET_DIR" "$CACHE_ROOT"

VERSION="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' "$ROOT/android/app/build.gradle.kts" | head -1)"
if [ -z "$VERSION" ]; then
    VERSION="dev"
fi

echo "Building Rust engine for Android $ABI (version=$VERSION)..."
echo "CARGO_TARGET_DIR=$TARGET_DIR CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-3}"

# F-Droid installs Rust with rustup, which Gradle's PATH does not include.
if ! command -v cargo >/dev/null && [ -x "$HOME/.cargo/bin/cargo" ]; then
    PATH="$HOME/.cargo/bin:$PATH"
fi
# Ensure the Android Rust target is installed.
rustup target add "$RUST_TARGET" >/dev/null 2>&1 || true

export ANDROID_NDK_HOME="$NDK"
export WEBTOR_APP_VERSION="$VERSION"
export APP_VERSION="$VERSION"
# 16 KB page alignment required by Google Play for Android 15+ devices.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-Wl,-z,max-page-size=16384"
# Keep build paths out of libengine.so so release builds are reproducible.
RUSTFLAGS="$RUSTFLAGS --remap-path-prefix=$ROOT=/src --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"

# Linker / C toolchain for crates with C code (e.g. ring).
export "${CARGO_TARGET_PREFIX}_LINKER=$CC"
export "CC_${RUST_TARGET//-/_}=$CC"
export "CXX_${RUST_TARGET//-/_}=$CXX"
export "AR_${RUST_TARGET//-/_}=$AR"
# Do not pass -D__ANDROID_API__=… here: the NDK *-android26-clang triple already
# defines it, and aws-lc-sys builds with -Werror/-Wmacro-redefined.
export CC="$CC"
export CXX="$CXX"
export AR="$AR"
export CARGO_TARGET_DIR="$TARGET_DIR"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"

cd "$ENGINE_RS"
# One Android release build at a time across ABIs/workers.
flock "$LOCK_FILE" cargo build --locked --release -p wt-engine --lib --target "$RUST_TARGET"

SRC_SO="$TARGET_DIR/$RUST_TARGET/release/libengine.so"
if [ ! -f "$SRC_SO" ]; then
    echo "missing build output: $SRC_SO" >&2
    exit 1
fi

cp "$SRC_SO" "$OUT_SO"
if [ -x "$STRIP_BIN" ]; then
    "$STRIP_BIN" --strip-unneeded "$OUT_SO"
fi

SIZE="$(stat -c%s "$OUT_SO")"
echo "Successfully built $OUT_SO ($SIZE bytes)"

# Keep the per-ABI target dir under the shared 4 GB budget when possible.
TARGET_BYTES="$(du -sb "$TARGET_DIR" 2>/dev/null | awk '{print $1}')"
if [ -n "${TARGET_BYTES:-}" ] && [ "$TARGET_BYTES" -gt $((4 * 1024 * 1024 * 1024)) ]; then
    echo "warning: $TARGET_DIR is larger than 4 GiB ($TARGET_BYTES bytes); consider cargo clean there" >&2
fi

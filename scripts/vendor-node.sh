#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="${1:-${NODEJS_MOBILE_SOURCE:-$ROOT/vendor/nodejs-mobile/source}}"
REVISION=959b6e8637c86fb1f63f1aaf72adc86c7e8c335d # nodejs-mobile v18.20.4
NDK="${ANDROID_NDK_HOME:-${ANDROID_HOME:?Set ANDROID_HOME}/ndk/26.1.10909125}"
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
JNI="$ROOT/android/app/src/main/jniLibs/arm64-v8a"
INC="$ROOT/android/app/src/main/cpp/include"

if [ ! -f "$SOURCE/configure.py" ]; then
  echo "Missing nodejs-mobile source at $SOURCE; see README.md build instructions." >&2
  exit 1
fi
SOURCE="$(cd "$SOURCE" && pwd)"
if [ "$(git -C "$SOURCE" rev-parse HEAD)" != "$REVISION" ]; then
  echo "Expected nodejs-mobile source revision $REVISION" >&2
  exit 1
fi

# Match upstream android-configure, invoking configure.py directly so Debian's
# Python 3.13 works with python3-setuptools providing distutils.
export PATH="$TOOLCHAIN/bin:$PATH"
export CC="$TOOLCHAIN/bin/aarch64-linux-android26-clang"
export CXX="$TOOLCHAIN/bin/aarch64-linux-android26-clang++"
export CC_host="${CC_host:-gcc}"
export CXX_host="${CXX_host:-g++}"
export GYP_DEFINES="target_arch=arm64 v8_target_arch=arm64 android_target_arch=arm64 host_os=linux OS=android ANDROID_NDK_ROOT=$NDK ANDROID_NDK_SYSROOT=$TOOLCHAIN/sysroot"
cd "$SOURCE"
python3 configure.py --dest-cpu=arm64 --dest-os=android --openssl-no-asm \
  --with-intl=none --cross-compiling --shared
make -j"${WEBTOR_BUILD_JOBS:-2}"
bash tools/copy_libnode_headers.sh android

LIBRARY=out/Release/lib.target/libnode.so
if [ ! -f "$LIBRARY" ]; then
  LIBRARY=out/Release/obj.target/libnode.so
fi
mkdir -p "$JNI" "$INC"
cp "$LIBRARY" "$JNI/libnode.so"
"$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$JNI/libnode.so"
cp -a out_android/libnode/include/node/. "$INC/"
mkdir -p "$ROOT/android/app/src/main/assets/licenses"
cp LICENSE "$ROOT/android/app/src/main/assets/licenses/nodejs-mobile.txt"
echo "Built libnode.so from $REVISION and staged headers."

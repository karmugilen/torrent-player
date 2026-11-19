#!/usr/bin/env bash
# Builds the built-in player's native libraries from source for arm64-v8a:
# dav1d, FFmpeg (LGPL build), FreeType, FriBidi, HarfBuzz, libass, libplacebo,
# mpv (-Dgpl=false) and the JNI wrapper libplayer.so (wrapper/, from
# libmpv-android 0.4.1, MIT). Based on libmpv-android's buildscripts.
#
# Sources go in android/mpv/deps/<name>. Missing ones are cloned at the pinned
# tags below; the F-Droid recipe fills deps/ from srclibs first, so its build
# fetches nothing. Results are copied to android/app/src/main/jniLibs/arm64-v8a.
#
# Needs ANDROID_NDK_HOME (NDK 28.2.13676358), meson, ninja, pkg-config, make,
# autoconf/automake/libtool (libass) and python3.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEPS="$HERE/deps"
WORK="$HERE/build/arm64-v8a"
PREFIX="$WORK/prefix"
OUT="$HERE/../app/src/main/jniLibs/arm64-v8a"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${ANDROID_HOME:-}/ndk/28.2.13676358}}"
[ -d "$NDK" ] || { echo "Android NDK not found; set ANDROID_NDK_HOME" >&2; exit 1; }

# Pinned versions (tag names). Keep in step with webtor.app.yml srclibs and NOTICE.md.
V_DAV1D=1.4.3
V_FFMPEG=n7.1
V_FREETYPE=VER-2-13-3
V_FRIBIDI=v1.0.16
V_HARFBUZZ=10.0.1
V_LIBASS=0.17.3
V_LIBPLACEBO=v7.349.0
V_MPV=v0.39.0

LIBS=(libavcodec.so libavdevice.so libavfilter.so libavformat.so libavutil.so
      libswresample.so libswscale.so libmpv.so libplayer.so libc++_shared.so)

API=26
TRIPLE=aarch64-linux-android
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
export PATH="$TOOLCHAIN/bin:$PATH"
CC="$TRIPLE$API-clang"
CXX="$TRIPLE$API-clang++"
CORES="${CORES:-$(nproc)}"

# Skip the build when nothing that affects it changed and the outputs exist.
STAMP_KEY="$(cat "$0" "$HERE"/wrapper/* | sha256sum | cut -d' ' -f1) $(basename "$NDK")"
complete() { for l in "${LIBS[@]}"; do [ -f "$OUT/$l" ] || return 1; done; }
if [ -f "$WORK/stamp" ] && [ "$(cat "$WORK/stamp")" = "$STAMP_KEY" ] && complete; then
    echo "Player libraries up to date"
    exit 0
fi

fetch() { # name url tag [recursive]
    [ -d "$DEPS/$1" ] && return 0
    echo "Fetching $1 $3"
    git clone -q --depth 1 --branch "$3" ${4:+--recurse-submodules --shallow-submodules} "$2" "$DEPS/$1"
}
mkdir -p "$DEPS"
fetch dav1d https://code.videolan.org/videolan/dav1d.git "$V_DAV1D"
fetch ffmpeg https://github.com/FFmpeg/FFmpeg.git "$V_FFMPEG"
fetch freetype https://gitlab.freedesktop.org/freetype/freetype.git "$V_FREETYPE"
fetch fribidi https://github.com/fribidi/fribidi.git "$V_FRIBIDI"
fetch harfbuzz https://github.com/harfbuzz/harfbuzz.git "$V_HARFBUZZ"
fetch libass https://github.com/libass/libass.git "$V_LIBASS"
fetch libplacebo https://code.videolan.org/videolan/libplacebo.git "$V_LIBPLACEBO" recursive
fetch mpv https://github.com/mpv-player/mpv.git "$V_MPV"

rm -rf "$WORK"
mkdir -p "$PREFIX" "$OUT"
# Flat prefix: /usr/local/lib from DESTDIR installs lands in $PREFIX/lib.
ln -s . "$PREFIX/usr"
ln -s . "$PREFIX/local"
export PKG_CONFIG_SYSROOT_DIR="$PREFIX"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
unset PKG_CONFIG_PATH
# Same paths for every build, so F-Droid's rebuild matches byte for byte.
export SOURCE_DATE_EPOCH=0
LDFLAGS_COMMON="-Wl,-O1,--icf=safe -Wl,-z,max-page-size=16384"

cat >"$WORK/cross.txt" <<EOF
[built-in options]
buildtype = 'release'
default_library = 'static'
wrap_mode = 'nodownload'
c_link_args = ['-Wl,-z,max-page-size=16384']
cpp_link_args = ['-Wl,-z,max-page-size=16384']
[binaries]
c = '$CC'
cpp = '$CXX'
ar = 'llvm-ar'
nm = 'llvm-nm'
strip = 'llvm-strip'
pkg-config = 'pkg-config'
[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
EOF

meson_build() { # name [options...]
    local name=$1; shift
    echo "Building $name"
    (cd "$DEPS/$name" &&
        env -u CC -u CXX meson setup "$WORK/$name" --cross-file "$WORK/cross.txt" "$@" >"$WORK/$name.log" 2>&1 &&
        ninja -C "$WORK/$name" -j"$CORES" >>"$WORK/$name.log" 2>&1 &&
        DESTDIR="$PREFIX" ninja -C "$WORK/$name" install >>"$WORK/$name.log" 2>&1) ||
        { tail -40 "$WORK/$name.log"; echo "$name failed; log: $WORK/$name.log" >&2; exit 1; }
}

meson_build dav1d -Denable_tests=false -Denable_tools=false -Db_lto=true -Dstack_alignment=16

echo "Building ffmpeg"
mkdir -p "$WORK/ffmpeg"
(cd "$WORK/ffmpeg" && "$DEPS/ffmpeg/configure" \
    --target-os=android --enable-cross-compile --cross-prefix=$TRIPLE- --cc="$CC" \
    --arch=aarch64 --cpu=armv8-a --pkg-config=pkg-config --nm=llvm-nm \
    --extra-cflags="-I$PREFIX/include" --extra-ldflags="-L$PREFIX/lib $LDFLAGS_COMMON" \
    --enable-jni --enable-mediacodec --enable-libdav1d --disable-vulkan \
    --disable-static --enable-shared \
    --disable-stripping --disable-doc --disable-programs \
    --disable-muxers --disable-encoders --disable-devices --disable-filters \
    --disable-v4l2-m2m >"$WORK/ffmpeg.log" 2>&1 &&
    make -j"$CORES" >>"$WORK/ffmpeg.log" 2>&1 &&
    make DESTDIR="$PREFIX" install >>"$WORK/ffmpeg.log" 2>&1) ||
    { tail -40 "$WORK/ffmpeg.log"; echo "ffmpeg failed; log: $WORK/ffmpeg.log" >&2; exit 1; }

meson_build freetype -Dharfbuzz=disabled -Dbrotli=disabled -Dbzip2=disabled -Dpng=disabled
meson_build fribidi -Dtests=false -Ddocs=false -Dbin=false
meson_build harfbuzz -Dtests=disabled -Ddocs=disabled -Dutilities=disabled -Dglib=disabled \
    -Dgobject=disabled -Dicu=disabled -Dcairo=disabled -Dchafa=disabled

echo "Building libass"
rm -rf "$WORK/libass-src"
cp -a "$DEPS/libass" "$WORK/libass-src"
(cd "$WORK/libass-src" && ./autogen.sh && mkdir -p _build && cd _build &&
    ../configure --host=$TRIPLE --with-pic --enable-static --disable-shared \
        --disable-require-system-font-provider --prefix=/usr/local \
        CC="$CC" CXX="$CXX" AR=llvm-ar RANLIB=llvm-ranlib &&
    make -j"$CORES" && make DESTDIR="$PREFIX" install) >"$WORK/libass.log" 2>&1 ||
    { tail -40 "$WORK/libass.log"; echo "libass failed; log: $WORK/libass.log" >&2; exit 1; }

meson_build libplacebo -Dvulkan=disabled -Ddemos=false -Dtests=false
# Static libplacebo needs libc++ when linked into libmpv.so.
sed -i '/^Libs:/ s|$| -lc++|' "$PREFIX/lib/pkgconfig/libplacebo.pc"

meson_build mpv --default-library shared -Dgpl=false -Dbuild-date=false \
    -Diconv=disabled -Dlua=disabled -Djavascript=disabled -Dlibmpv=true -Dcplayer=false \
    -Dmanpage-build=disabled -Dhtml-build=disabled -Dpdf-build=disabled

echo "Building libplayer (JNI wrapper)"
"$CXX" -shared -fPIC -O2 -std=c++11 -Werror -I"$PREFIX/include" \
    "$HERE"/wrapper/*.cpp -L"$PREFIX/lib" -lmpv -lavcodec -llog \
    $LDFLAGS_COMMON -Wl,-soname,libplayer.so -o "$WORK/libplayer.so"

for l in "${LIBS[@]}"; do
    case "$l" in
        libplayer.so) src="$WORK/libplayer.so" ;;
        libc++_shared.so) src="$TOOLCHAIN/sysroot/usr/lib/$TRIPLE/libc++_shared.so" ;;
        *) src="$PREFIX/lib/$l" ;;
    esac
    cp -L "$src" "$OUT/$l"
done
echo "$STAMP_KEY" >"$WORK/stamp"
ls -l "${LIBS[@]/#/$OUT/}"

# Third-party notices

Torrent Player's own code is under the [MIT License](LICENSE). The torrent engine in `engine-rs/` is under the
[Mozilla Public License 2.0](engine-rs/LICENSE); see [engine-rs/NOTICE.md](engine-rs/NOTICE.md). The Rust crates
linked into the engine are listed in [rust-dependencies.txt](android/app/src/main/assets/licenses/rust-dependencies.txt),
and the peer map data in [peer-map-data.txt](android/app/src/main/assets/licenses/peer-map-data.txt).

## Built-in video player

Since 2.4.4 the APK includes native libraries that [android/mpv/build.sh](android/mpv/build.sh) builds from
source at the tags below. mpv is built with `-Dgpl=false` and FFmpeg without `--enable-gpl` or
`--enable-version3`, so every library is under a license that allows linking from MIT code. Each is a separate
shared library (`lib/arm64-v8a/*.so`) that can be replaced.

| Project | Version | License | Source |
| --- | --- | --- | --- |
| mpv (`libmpv.so`) | v0.39.0 | LGPL-2.1-or-later | https://github.com/mpv-player/mpv |
| FFmpeg (`libavcodec`, `libavformat`, `libavutil`, `libavfilter`, `libavdevice`, `libswscale`, `libswresample`) | n7.1 | LGPL-2.1-or-later | https://github.com/FFmpeg/FFmpeg |
| libplacebo (static, in `libmpv.so`) | v7.349.0 | LGPL-2.1-or-later | https://code.videolan.org/videolan/libplacebo |
| FriBidi (static, in `libmpv.so`) | v1.0.16 | LGPL-2.1-or-later | https://github.com/fribidi/fribidi |
| libass (static, in `libmpv.so`) | 0.17.3 | ISC | https://github.com/libass/libass |
| FreeType (static, in `libmpv.so`) | 2.13.3 | FreeType License (FTL) | https://gitlab.freedesktop.org/freetype/freetype |
| HarfBuzz (static, in `libmpv.so`) | 10.0.1 | MIT ("Old MIT") | https://github.com/harfbuzz/harfbuzz |
| dav1d (static, in `libavcodec.so`) | 1.4.3 | BSD-2-Clause | https://code.videolan.org/videolan/dav1d |
| JNI wrapper (`libplayer.so`, `dev.jdtech.mpv.MPVLib`) | libmpv-android 0.4.1 | MIT | https://github.com/jarnedemeulemeester/libmpv-android, license in [android/mpv/LICENSE.libmpv-android](android/mpv/LICENSE.libmpv-android) |
| LLVM libc++ (`libc++_shared.so`) | Android NDK r26b | Apache-2.0 WITH LLVM-exception | https://github.com/llvm/llvm-project |

The LGPL libraries are unmodified. To rebuild them, or to build the app with modified versions, run
`android/mpv/build.sh` (it fetches the tags above into `android/mpv/deps/`, or uses what is already there) and
then build the APK as described in the README.

License texts: [LGPL-2.1](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt),
[FreeType License](https://gitlab.freedesktop.org/freetype/freetype/-/blob/master/docs/FTL.TXT),
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0.txt).

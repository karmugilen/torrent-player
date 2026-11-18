# Third-party notices

Torrent Player's own code is under the [MIT License](LICENSE). The torrent engine in `engine-rs/` is under the
[Mozilla Public License 2.0](engine-rs/LICENSE); see [engine-rs/NOTICE.md](engine-rs/NOTICE.md). The Rust crates
linked into the engine are listed in [rust-dependencies.txt](android/app/src/main/assets/licenses/rust-dependencies.txt),
and the peer map data in [peer-map-data.txt](android/app/src/main/assets/licenses/peer-map-data.txt).

## Built-in video player

Since 2.4.4 the APK includes the prebuilt Android package
[`dev.jdtech.mpv:libmpv:0.4.1`](https://github.com/jarnedemeulemeester/libmpv-android) (wrapper code under the MIT
License). It contains native libraries built from these projects:

| Project | Version | License | Source |
| --- | --- | --- | --- |
| mpv | 0.39.0, built with the `gpl` option | GPL-2.0-or-later | https://github.com/mpv-player/mpv |
| FFmpeg (libavcodec, libavformat, libavutil, libavfilter, libavdevice, libswscale, libswresample, libpostproc) | 7.1 (n7.1) | GPL-3.0-or-later, as configured with `--enable-gpl --enable-version3` | https://ffmpeg.org/download.html |
| libass | bundled | ISC | https://github.com/libass/libass |
| FreeType | bundled | FreeType License (FTL) | https://freetype.org |
| HarfBuzz | bundled | MIT ("Old MIT") | https://github.com/harfbuzz/harfbuzz |
| FriBidi | bundled | LGPL-2.1-or-later | https://github.com/fribidi/fribidi |
| libplacebo | bundled | LGPL-2.1-or-later | https://code.videolan.org/videolan/libplacebo |
| dav1d | bundled | BSD-2-Clause | https://code.videolan.org/videolan/dav1d |
| Mbed TLS | bundled | Apache-2.0 | https://github.com/Mbed-TLS/mbedtls |
| Lua | bundled | MIT | https://www.lua.org |
| LLVM libc++ (`libc++_shared.so`) | NDK | Apache-2.0 WITH LLVM-exception | https://github.com/llvm/llvm-project |

Because mpv and FFmpeg are built as GPL, the APK as a whole is distributed under the terms of the
GNU General Public License version 3. The app's own MIT and MPL-2.0 code stays under those licenses.

Corresponding source for the native libraries: the build scripts and pinned versions are in
[libmpv-android at tag 0.4.1](https://github.com/jarnedemeulemeester/libmpv-android/tree/0.4.1/buildscripts).
On request, the maintainer will provide the complete corresponding source for these libraries as shipped in
any Torrent Player release, for at least three years after that release: open an issue at
https://github.com/karmugilen/torrent-player/issues.

FFmpeg configure line (arm64-v8a):

```
--target-os=android --enable-cross-compile --arch=aarch64 --cpu=armv8-a --enable-jni --enable-mediacodec
--enable-mbedtls --enable-libdav1d --disable-vulkan --disable-static --enable-shared --enable-gpl
--enable-version3 --disable-stripping --disable-doc --disable-programs --disable-muxers --disable-encoders
--disable-devices --disable-filters --disable-v4l2-m2m
```

License texts: [GPL-3.0](https://www.gnu.org/licenses/gpl-3.0.txt),
[GPL-2.0](https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt),
[LGPL-2.1](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt),
[Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0.txt).

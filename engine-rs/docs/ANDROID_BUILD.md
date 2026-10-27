# Building the Android APK with Go or Rust engine

The Android app loads `libengine.so` via `EngineHost.kt` (`System.loadLibrary("engine")`).
Either the Go engine (`engine-go/`) or the Rust engine (`engine-rs/crates/wt-engine`) can
produce that library. Select with a Gradle property.

## Requirements

- JDK 17 or 21 (`JAVA_HOME=/usr/lib/jvm/java-21-openjdk` on this machine)
- Android SDK + NDK **26.1.10909125** (`ANDROID_HOME`, `ANDROID_NDK_HOME`)
- For Rust: Rust toolchain with `aarch64-linux-android` (and `x86_64-linux-android` for the emulator)
- Disk: keep ≥25 GB free; Rust Android builds write under `/home/kar/.cache/wt-targets/android-<abi>` (never `/tmp`)

## Go engine (default)

```sh
export JAVA_HOME=/usr/lib/jvm/java-21-openjdk
export ANDROID_HOME=/home/kar/Android/Sdk
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/26.1.10909125"

./android/gradlew -p android :app:assembleDebug --no-daemon \
  -Dorg.gradle.jvmargs=-Xmx2g
```

Equivalent explicit form:

```sh
./android/gradlew -p android :app:assembleDebug -Pwebtor.engine=go --no-daemon \
  -Dorg.gradle.jvmargs=-Xmx2g
```

This runs `engine-go/build.sh` and writes `android/app/src/main/jniLibs/arm64-v8a/libengine.so`.

## Rust engine

```sh
export JAVA_HOME=/usr/lib/jvm/java-21-openjdk
export ANDROID_HOME=/home/kar/Android/Sdk
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/26.1.10909125"

./android/gradlew -p android :app:assembleDebug -Pwebtor.engine=rust --no-daemon \
  -Dorg.gradle.jvmargs=-Xmx2g
```

This runs `engine-rs/build-android.sh` (NDK clang linker, 16 KiB page size, strip) into the
same `jniLibs/arm64-v8a/libengine.so` path. Cargo uses
`CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/android-arm64-v8a` with `CARGO_BUILD_JOBS=3`
and a flock so only one Android release cargo build runs at a time.

Standalone (without Gradle):

```sh
ENGINE_ABI=arm64-v8a bash engine-rs/build-android.sh
# or emulator:
ENGINE_ABI=x86_64 bash engine-rs/build-android.sh
```

## Emulator (x86_64)

Add `-Pwebtor.emulator=true` so Gradle also builds/packages `jniLibs/x86_64/libengine.so`
and adds the `x86_64` ABI filter:

```sh
./android/gradlew -p android :app:assembleDebug \
  -Pwebtor.engine=rust -Pwebtor.emulator=true --no-daemon \
  -Dorg.gradle.jvmargs=-Xmx2g
```

## Verify the APK contains the native library

```sh
APK=android/app/build/outputs/apk/debug/app-debug.apk
unzip -l "$APK" | grep 'lib/.*/libengine.so'
```

## Unit tests

Unit tests do not load the native `.so`. Prefer the Go default so they stay fast:

```sh
./android/gradlew -p android :app:testDebugUnitTest --no-daemon \
  -Dorg.gradle.jvmargs=-Xmx2g
```

## Host binary (`engine-host`)

Desktop control binary (port of `engine-go/main_host.go`):

```sh
cd engine-rs
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-engine-jni CARGO_BUILD_JOBS=3 \
  cargo build -p wt-engine --bin engine-host

# Run (control HTTP on 127.0.0.1:18080 by default)
/home/kar/.cache/wt-targets/wt-engine-jni/debug/engine-host \
  -port 18080 -path /tmp/webtorrent -max-peers 55

curl -s http://127.0.0.1:18080/stats
curl -s -X POST http://127.0.0.1:18080/shutdown
```

Flags: `-port`/`--port` (18080), `-path`/`--path` (`/tmp/webtorrent`),
`-max-peers`/`--max-peers` (55). The host starts `EngineServer`, media streaming,
and `ctl_http` on localhost; it exits on SIGINT/SIGTERM or `POST /shutdown`.

## RAM / concurrency

- One Gradle build at a time; use `--no-daemon` and `-Xmx2g` (not higher).
- If configuration-cache complains about Exec engine tasks, add `--no-configuration-cache`.
- One heavy Rust Android release build at a time (`flock` in `build-android.sh`).
- Keep each `/home/kar/.cache/wt-targets/android-*` tree under ~4 GB (`cargo clean` if needed).

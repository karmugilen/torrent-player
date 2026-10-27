# Android Rust engine integration — acceptance

## Results
- assembleDebug -Pwebtor.engine=rust: SUCCESS
- assembleDebug (default go): SUCCESS
- testDebugUnitTest (go default): SUCCESS
- Rust APK contains lib/arm64-v8a/libengine.so (unzip -l)
- Go APK contains lib/arm64-v8a/libengine.so

## libengine.so sizes (arm64-v8a)
| Engine | Bytes |
|---|---|
| Go | 22170160 |
| Rust | 2194912 |

Rust is smaller by 19975248 bytes.

## Host binary
- engine-host wired: EngineServer::new -> start_streaming -> ctl_http::serve
- Verified: curl http://127.0.0.1:18081/stats and POST /shutdown

## Files
- android/app/build.gradle.kts (engine tasks + -Pwebtor.engine)
- engine-rs/build-android.sh
- engine-rs/crates/wt-engine/src/ctl_http.rs
- engine-rs/crates/wt-engine/src/bin/main_host.rs
- engine-rs/crates/wt-engine/src/lib.rs (pub mod ctl_http)
- engine-rs/docs/ANDROID_BUILD.md

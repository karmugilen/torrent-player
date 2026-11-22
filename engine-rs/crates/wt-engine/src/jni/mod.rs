//! JNI exports matching `engine-go/main.go` and `EngineHost.kt`.
//!
//! Nine `Java_webtor_app_EngineHost_*` symbols are exported for
//! `System.loadLibrary("engine")`. A single global [`EngineHostApi`] is
//! installed by `startEngine` (via a registered factory or the unwired stub).

pub mod host_api;
pub mod server_impl;

pub use host_api::EngineHostApi;
pub use server_impl::AndroidEngineHost;

/// App version stamped by `build-android.sh` (`WEBTOR_APP_VERSION`), else `"dev"`.
pub fn webtor_app_version() -> &'static str {
    option_env!("WEBTOR_APP_VERSION").unwrap_or("dev")
}

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

use jni::objects::{JByteArray, JObject, JString};
use jni::sys::{jbyteArray, jint, jlong};
use jni::JNIEnv;

/// Factory that builds the [`EngineHostApi`] implementation used on Android.
///
/// Registered once from `lib.rs` so `startEngine` can construct the
/// `EngineServer`-backed host. When unset, an unwired stub
/// is used and `start_engine` returns `1`.
pub type EngineHostFactory = fn() -> Arc<dyn EngineHostApi>;

static ENGINE_FACTORY: OnceLock<EngineHostFactory> = OnceLock::new();
static ENGINE: OnceLock<Arc<dyn EngineHostApi>> = OnceLock::new();

/// Register the host-api factory. First registration wins.
pub fn register_engine_factory(factory: EngineHostFactory) -> bool {
    ENGINE_FACTORY.set(factory).is_ok()
}

/// Install a concrete host API (tests / early wiring). First call wins.
pub fn install_engine(api: Arc<dyn EngineHostApi>) -> bool {
    ENGINE.set(api).is_ok()
}

/// Current global engine, if `startEngine` (or [`install_engine`]) has run.
pub fn current_engine() -> Option<&'static Arc<dyn EngineHostApi>> {
    ENGINE.get()
}

fn engine_or_init() -> &'static Arc<dyn EngineHostApi> {
    ENGINE.get_or_init(|| match ENGINE_FACTORY.get() {
        Some(factory) => factory(),
        None => Arc::new(AndroidEngineHost::new()) as Arc<dyn EngineHostApi>,
    })
}

pub(crate) fn log_info(msg: &str) {
    #[cfg(target_os = "android")]
    {
        log::info!(target: "webtor-engine", "{msg}");
    }
    #[cfg(not(target_os = "android"))]
    {
        eprintln!("[webtor-engine] {msg}");
    }
}

pub(crate) fn log_error(msg: &str) {
    #[cfg(target_os = "android")]
    {
        log::error!(target: "webtor-engine", "{msg}");
    }
    #[cfg(not(target_os = "android"))]
    {
        eprintln!("[webtor-engine] ERROR {msg}");
    }
}

#[cfg(target_os = "android")]
fn init_android_logging() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("webtor-engine"),
        );
        // catch_unwind keeps the process alive but drops the message; log it.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}");
            previous(info);
        }));
    });
}

#[cfg(not(target_os = "android"))]
fn init_android_logging() {}

fn java_string(env: &mut JNIEnv<'_>, value: JString<'_>) -> String {
    if value.is_null() {
        return String::new();
    }
    env.get_string(&value).map(|s| s.into()).unwrap_or_default()
}

fn java_bytes(env: &mut JNIEnv<'_>, value: &JByteArray<'_>) -> Vec<u8> {
    let len = match env.get_array_length(value) {
        Ok(n) => n as usize,
        Err(_) => return Vec::new(),
    };
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0i8; len];
    if env.get_byte_array_region(value, 0, &mut buf).is_err() {
        return Vec::new();
    }
    buf.into_iter().map(|b| b as u8).collect()
}

fn new_java_bytes(env: &mut JNIEnv<'_>, data: &[u8]) -> jbyteArray {
    match env.byte_array_from_slice(data) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

const MAX_READ_SIZE: jint = 1024 * 1024;

/// `Java_webtor_app_EngineHost_startEngine`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_startEngine(
    mut env: JNIEnv,
    _host: JObject,
    download_dir: JByteArray,
    max_peers: jint,
) -> jint {
    let result = catch_unwind(AssertUnwindSafe(|| {
        init_android_logging();
        log_info("initializing native Rust engine");
        let dir = java_bytes(&mut env, &download_dir);
        engine_or_init().start_engine(&dir, max_peers)
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            log_error("startEngine panicked");
            1
        }
    }
}

/// `Java_webtor_app_EngineHost_requestEngine`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_requestEngine(
    mut env: JNIEnv,
    _host: JObject,
    method: JString,
    path: JString,
    body: JByteArray,
) -> jbyteArray {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let method = java_string(&mut env, method);
        let path = java_string(&mut env, path);
        let body = java_bytes(&mut env, &body);
        let payload = match current_engine() {
            Some(engine) => engine.request_engine(&method, &path, &body),
            None => b"503\n{\"error\":\"engine is starting\"}".to_vec(),
        };
        new_java_bytes(&mut env, &payload)
    }));
    match result {
        Ok(arr) => arr,
        Err(_) => {
            log_error("requestEngine panicked");
            new_java_bytes(&mut env, b"500\n{\"error\":\"native panic\"}")
        }
    }
}

/// `Java_webtor_app_EngineHost_waitForEngineEvent`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_waitForEngineEvent(
    _env: JNIEnv,
    _host: JObject,
    after_version: jlong,
    timeout_millis: jlong,
) -> jlong {
    let result = catch_unwind(AssertUnwindSafe(|| match current_engine() {
        Some(engine) => engine.wait_for_engine_event(after_version, timeout_millis),
        None => {
            std::thread::sleep(std::time::Duration::from_millis(250));
            after_version
        }
    }));
    match result {
        Ok(v) => v,
        Err(_) => {
            log_error("waitForEngineEvent panicked");
            after_version
        }
    }
}

/// `Java_webtor_app_EngineHost_openPlayback`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_openPlayback(
    mut env: JNIEnv,
    _host: JObject,
    id: JString,
    index: jint,
) -> jlong {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(engine) = current_engine() else {
            return 0;
        };
        let id = java_string(&mut env, id);
        engine.open_playback(&id, index)
    }));
    match result {
        Ok(handle) => handle,
        Err(_) => {
            log_error("openPlayback panicked");
            0
        }
    }
}

/// `Java_webtor_app_EngineHost_readPlayback`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_readPlayback(
    env: JNIEnv,
    _host: JObject,
    handle: jlong,
    offset: jlong,
    size: jint,
    target: JByteArray,
) -> jint {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Ok(array_len) = env.get_array_length(&target) else {
            return -1;
        };
        if !(0..=MAX_READ_SIZE).contains(&size) || size > array_len {
            return -1;
        }
        let Some(engine) = current_engine() else {
            return -1;
        };
        let mut buf = vec![0u8; size as usize];
        let n = engine.read_playback(handle, offset, &mut buf);
        if n > 0 {
            // One buffer: reinterpret u8 → i8 for SetByteArrayRegion (same layout).
            let signed =
                unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<i8>(), n as usize) };
            if env.set_byte_array_region(&target, 0, signed).is_err() {
                return -1;
            }
        }
        n
    }));
    match result {
        Ok(n) => n,
        Err(_) => {
            log_error("readPlayback panicked");
            -1
        }
    }
}

/// `Java_webtor_app_EngineHost_closePlayback`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_closePlayback(
    _env: JNIEnv,
    _host: JObject,
    handle: jlong,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(engine) = current_engine() {
            engine.close_playback(handle);
        }
    }));
}

/// `Java_webtor_app_EngineHost_openPreview`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_openPreview(
    mut env: JNIEnv,
    _host: JObject,
    id: JString,
    index: jint,
) -> jlong {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(engine) = current_engine() else {
            return 0;
        };
        let id = java_string(&mut env, id);
        engine.open_preview(&id, index)
    }));
    match result {
        Ok(handle) => handle,
        Err(_) => {
            log_error("openPreview panicked");
            0
        }
    }
}

/// `Java_webtor_app_EngineHost_readPreview`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_readPreview(
    env: JNIEnv,
    _host: JObject,
    handle: jlong,
    offset: jlong,
    size: jint,
    target: JByteArray,
) -> jint {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Ok(array_len) = env.get_array_length(&target) else {
            return -1;
        };
        if !(0..=MAX_READ_SIZE).contains(&size) || size > array_len {
            return -1;
        }
        let Some(engine) = current_engine() else {
            return -1;
        };
        let mut buf = vec![0u8; size as usize];
        let n = engine.read_preview(handle, offset, &mut buf);
        if n < 0 {
            return -1;
        }
        if n > 0 {
            let signed =
                unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<i8>(), n as usize) };
            if env.set_byte_array_region(&target, 0, signed).is_err() {
                return -1;
            }
        }
        n
    }));
    match result {
        Ok(n) => n,
        Err(_) => {
            log_error("readPreview panicked");
            -1
        }
    }
}

/// `Java_webtor_app_EngineHost_closePreview`
#[no_mangle]
pub extern "system" fn Java_webtor_app_EngineHost_closePreview(
    _env: JNIEnv,
    _host: JObject,
    handle: jlong,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(engine) = current_engine() {
            engine.close_preview(handle);
        }
    }));
}

/// Exported symbol names expected in `libengine.so` (for tests).
pub const JNI_EXPORT_SYMBOLS: &[&str] = &[
    "Java_webtor_app_EngineHost_startEngine",
    "Java_webtor_app_EngineHost_requestEngine",
    "Java_webtor_app_EngineHost_waitForEngineEvent",
    "Java_webtor_app_EngineHost_openPlayback",
    "Java_webtor_app_EngineHost_readPlayback",
    "Java_webtor_app_EngineHost_closePlayback",
    "Java_webtor_app_EngineHost_openPreview",
    "Java_webtor_app_EngineHost_readPreview",
    "Java_webtor_app_EngineHost_closePreview",
];

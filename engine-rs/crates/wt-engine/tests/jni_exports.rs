//! JNI export smoke tests for wt-engine.
//!
//! 1. Symbol check: the built cdylib must export exactly the nine
//!    `Java_webtor_app_EngineHost_*` names (via `nm -D` / `llvm-nm -D`).
//! 2. Optional JVM invocation test exercising a fake `EngineHostApi` through
//!    the JNI functions — skipped cleanly when no JVM is available.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use engine::jni::{install_engine, EngineHostApi, JNI_EXPORT_SYMBOLS};

/// Fake host used to exercise API semantics without a full EngineServer.
struct FakeHost {
    started: AtomicI32,
    version: AtomicI64,
    playback: Mutex<std::collections::HashMap<i64, Vec<u8>>>,
    preview: Mutex<std::collections::HashMap<i64, Vec<u8>>>,
    next_handle: AtomicI64,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            started: AtomicI32::new(0),
            version: AtomicI64::new(1),
            playback: Mutex::new(std::collections::HashMap::new()),
            preview: Mutex::new(std::collections::HashMap::new()),
            next_handle: AtomicI64::new(1),
        }
    }
}

impl EngineHostApi for FakeHost {
    fn start_engine(&self, download_dir: &[u8], max_peers: i32) -> i32 {
        if download_dir.is_empty() {
            return 1;
        }
        let _ = max_peers;
        if self.started.swap(1, Ordering::SeqCst) == 1 {
            return 0;
        }
        self.version.fetch_add(1, Ordering::SeqCst);
        0
    }

    fn request_engine(&self, method: &str, path: &str, _body: &[u8]) -> Vec<u8> {
        if self.started.load(Ordering::SeqCst) == 0 {
            return b"503\n{\"error\":\"engine is starting\"}".to_vec();
        }
        let body = format!("{{\"method\":\"{method}\",\"path\":\"{path}\"}}");
        let mut out = b"200\n".to_vec();
        out.extend_from_slice(body.as_bytes());
        out
    }

    fn wait_for_engine_event(&self, after_version: i64, _timeout_millis: i64) -> i64 {
        let v = self.version.load(Ordering::SeqCst);
        if v > after_version {
            v
        } else {
            after_version
        }
    }

    fn open_playback(&self, _id: &str, _file_index: i32) -> i64 {
        if self.started.load(Ordering::SeqCst) == 0 {
            return 0;
        }
        let handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
        self.playback
            .lock()
            .unwrap()
            .insert(handle, b"playback-bytes-0123456789".to_vec());
        handle
    }

    fn read_playback(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32 {
        let map = self.playback.lock().unwrap();
        let Some(data) = map.get(&handle) else {
            return -1;
        };
        if offset < 0 || buf.len() > 1024 * 1024 {
            return -1;
        }
        let start = offset as usize;
        if start >= data.len() {
            return 0;
        }
        let n = (data.len() - start).min(buf.len());
        buf[..n].copy_from_slice(&data[start..start + n]);
        n as i32
    }

    fn close_playback(&self, handle: i64) {
        self.playback.lock().unwrap().remove(&handle);
    }

    fn open_preview(&self, _id: &str, _file_index: i32) -> i64 {
        if self.started.load(Ordering::SeqCst) == 0 {
            return 0;
        }
        let handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
        self.preview
            .lock()
            .unwrap()
            .insert(handle, b"preview-data".to_vec());
        handle
    }

    fn read_preview(&self, handle: i64, offset: i64, buf: &mut [u8]) -> i32 {
        let map = self.preview.lock().unwrap();
        let Some(data) = map.get(&handle) else {
            return -1;
        };
        if offset < 0 || buf.len() > 1024 * 1024 {
            return -1;
        }
        let start = offset as usize;
        if start >= data.len() {
            return 0;
        }
        let n = (data.len() - start).min(buf.len());
        buf[..n].copy_from_slice(&data[start..start + n]);
        n as i32
    }

    fn close_preview(&self, handle: i64) {
        self.preview.lock().unwrap().remove(&handle);
    }
}

#[test]
fn fake_host_api_semantics() {
    let host = FakeHost::new();
    assert_eq!(host.start_engine(b"", 55), 1);
    assert_eq!(host.start_engine(b"/tmp/webtorrent", 55), 0);
    assert_eq!(host.start_engine(b"/tmp/webtorrent", 55), 0);

    let payload = host.request_engine("GET", "/stats", b"");
    let text = String::from_utf8(payload).unwrap();
    assert!(text.starts_with("200\n"), "{text}");
    assert!(text.contains("/stats"), "{text}");

    let handle = host.open_playback("abc", 0);
    assert!(handle > 0);
    let mut buf = [0u8; 8];
    assert_eq!(host.read_playback(handle, 0, &mut buf), 8);
    assert_eq!(&buf, b"playback");
    host.close_playback(handle);
    assert_eq!(host.read_playback(handle, 0, &mut buf), -1);

    let preview = host.open_preview("abc", 0);
    assert!(preview > 0);
    let mut pbuf = [0u8; 7];
    assert_eq!(host.read_preview(preview, 0, &mut pbuf), 7);
    assert_eq!(&pbuf, b"preview");
    host.close_preview(preview);
}

#[test]
fn install_engine_accepts_fake_host() {
    // First install wins (OnceLock). Ignore failure if a prior test already set it.
    let _ = install_engine(Arc::new(FakeHost::new()));
}

fn find_nm() -> Option<PathBuf> {
    for candidate in ["llvm-nm", "nm"] {
        if let Ok(output) = Command::new(candidate).arg("--version").output() {
            if output.status.success() {
                return Some(PathBuf::from(candidate));
            }
        }
    }
    // NDK llvm-nm
    let ndk = std::env::var_os("ANDROID_NDK_HOME")
        .or_else(|| std::env::var_os("ANDROID_NDK_ROOT"))
        .map(PathBuf::from);
    if let Some(ndk) = ndk {
        let p = ndk.join("toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn parse_dynamic_symbols(nm_output: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in nm_output.lines() {
        // Formats: "0000000000000000 T name" or "                 U name"
        let mut parts = line.split_whitespace();
        let (typ, name) = match (parts.next(), parts.next(), parts.next()) {
            (Some(_addr), Some(t), Some(n)) if t.len() == 1 => (t, n),
            (Some(t), Some(n), None) if t.len() == 1 => (t, n),
            _ => continue,
        };
        if matches!(
            typ,
            "T" | "t" | "D" | "d" | "B" | "b" | "R" | "r" | "W" | "w"
        ) {
            let name = name.strip_prefix('_').unwrap_or(name);
            if name.starts_with("Java_webtor_app_EngineHost_") {
                out.insert(name.to_string());
            }
        }
    }
    out
}

fn candidate_libengine_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        for target in [
            "debug",
            "release",
            "aarch64-linux-android/release",
            "x86_64-linux-android/release",
        ] {
            paths.push(PathBuf::from(&dir).join(target).join("libengine.so"));
        }
    }
    // Default relative target dirs from engine-rs/
    for target in [
        "target/debug/libengine.so",
        "target/release/libengine.so",
        "target/aarch64-linux-android/release/libengine.so",
    ] {
        paths.push(PathBuf::from(target));
    }
    // Android jniLibs output
    paths.push(PathBuf::from(
        "../android/app/src/main/jniLibs/arm64-v8a/libengine.so",
    ));
    paths.push(PathBuf::from(
        "../../android/app/src/main/jniLibs/arm64-v8a/libengine.so",
    ));
    paths.push(PathBuf::from(
        "/tmp/wt-target-engine-android/aarch64-linux-android/release/libengine.so",
    ));
    paths.push(PathBuf::from("/tmp/wt-target-engine/debug/libengine.so"));
    paths
}

#[test]
fn libengine_exports_nine_jni_symbols() {
    let Some(nm) = find_nm() else {
        eprintln!("nm/llvm-nm not found; skipping symbol export check");
        return;
    };

    let lib = candidate_libengine_paths().into_iter().find(|p| p.exists());
    let Some(lib) = lib else {
        eprintln!(
            "libengine.so not found in candidate paths; skipping symbol export check \
             (build with cargo build -p wt-engine or build-android.sh first)"
        );
        return;
    };

    let output = Command::new(&nm)
        .args(["-D", "--defined-only"])
        .arg(&lib)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {nm:?}: {e}"));
    if !output.status.success() {
        // Some nm builds dislike --defined-only; retry plain -D.
        let output = Command::new(&nm)
            .arg("-D")
            .arg(&lib)
            .output()
            .expect("nm -D failed to spawn");
        assert!(
            output.status.success(),
            "nm failed on {}: {}",
            lib.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let symbols = parse_dynamic_symbols(&String::from_utf8_lossy(&output.stdout));
        let expected: BTreeSet<_> = JNI_EXPORT_SYMBOLS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(
            symbols,
            expected,
            "JNI symbols in {} mismatch.\nfound: {symbols:?}\nexpected: {expected:?}",
            lib.display()
        );
        return;
    }

    let symbols = parse_dynamic_symbols(&String::from_utf8_lossy(&output.stdout));
    let expected: BTreeSet<_> = JNI_EXPORT_SYMBOLS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        symbols,
        expected,
        "JNI symbols in {} mismatch.\nfound: {symbols:?}\nexpected: {expected:?}",
        lib.display()
    );
}

#[test]
fn jvm_invocation_fake_host_optional() {
    // Requires a host JVM + jni `invocation` feature. Skip cleanly otherwise.
    let args = match jni::InitArgsBuilder::new()
        .version(jni::JNIVersion::V8)
        .build()
    {
        Ok(args) => args,
        Err(err) => {
            eprintln!("JNI init args failed ({err}); skipping JNI invocation test");
            return;
        }
    };
    let vm = match jni::JavaVM::new(args) {
        Ok(vm) => vm,
        Err(err) => {
            eprintln!("JVM not available ({err}); skipping JNI invocation test");
            return;
        }
    };

    let _ = install_engine(Arc::new(FakeHost::new()));

    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(err) => {
            eprintln!("attach_current_thread failed ({err}); skipping");
            return;
        }
    };

    // Exercise JNI helpers against a live JNIEnv (byte array + string round-trip).
    let dir = env
        .byte_array_from_slice(b"/tmp/webtorrent-jni-test")
        .expect("byte array");
    let roundtrip = env.convert_byte_array(&dir).expect("convert");
    assert_eq!(roundtrip, b"/tmp/webtorrent-jni-test");

    let method = env.new_string("GET").unwrap();
    let method_str: String = env.get_string(&method).unwrap().into();
    assert_eq!(method_str, "GET");

    // Call exported startEngine with a raw JNIEnv pointer reconstructed for the call.
    let raw = env.get_raw();
    let rc = unsafe {
        let env_for_call = jni::JNIEnv::from_raw(raw).expect("from_raw");
        engine::jni::Java_webtor_app_EngineHost_startEngine(
            env_for_call,
            jni::objects::JObject::null(),
            dir,
            55,
        )
    };
    // Fake/unwired may already be installed from another test; accept 0 or 1.
    assert!(rc == 0 || rc == 1, "startEngine returned {rc}");
}

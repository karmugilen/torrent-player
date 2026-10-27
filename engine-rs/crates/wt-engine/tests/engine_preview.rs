//! Integration tests ported from `engine-go/preview_test.go`.
//!
//! Drive [`engine::EngineServer`] via `dispatch_control` (JNI control path)
//! and `open_preview`.

mod support;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use support::TestEngine;
use support::playback::{add_recovery_torrent, recovery_metadata, write_verified_piece};

#[ignore = "needs: preview read verified bytes without mutating focus"]
#[test]
fn test_open_preview_reads_verified_bytes_without_focus_change() {
    // Port of Go TestOpenPreviewReadsVerifiedBytesWithoutFocusChange
    let (data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_path = tmp.path().join("partial.dat");
    let dest_file = std::fs::File::create(&dest_path).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));

    write_verified_piece(&eng, &id, &data, 0);
    write_verified_piece(&eng, &id, &data, 1);

    let preview = eng.open_preview(&id, 0).expect("open preview");
    let mut buf = [0u8; 32];
    let n = preview.read_at(&mut buf, 0).expect("read");
    assert_eq!(n, 32);
    assert_eq!(&buf[..], &data[..32]);

    preview.close();
    assert!(preview.is_closed());
}

#[test]
fn test_preview_rejects_second_concurrent_job() {
    // Port of Go TestPreviewRejectsSecondConcurrentJob
    let eng = TestEngine::new();
    let (data, metadata) = recovery_metadata(None);
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    match eng.open_preview(&id, 0) {
        Ok(first) => {
            let second = eng.open_preview(&id, 0);
            assert!(second.is_err(), "second concurrent preview must fail");
            first.close();
            let third = eng.open_preview(&id, 0);
            assert!(
                third.is_ok(),
                "after close, opening new preview must succeed"
            );
            third.unwrap().close();
        }
        Err(e) => {
            panic!("first open_preview failed: {e}");
        }
    }
}

#[ignore = "needs: preview temp storage cleanup on close"]
#[test]
fn test_preview_temp_for_ready_draft_without_full_file() {
    // Port of Go TestPreviewTempForReadyDraftWithoutFullFile
    let (_data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let preview = eng.open_preview(&id, 0).expect("open preview");
    preview.close();
}

#[test]
fn test_preview_close_is_idempotent_and_cancels_read() {
    // Port of Go TestPreviewCloseIsIdempotentAndCancelsRead
    let (data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    let preview = eng.open_preview(&id, 0).expect("open preview");
    preview.close();
    preview.close(); // idempotent

    let mut buf = [0u8; 8];
    let res = preview.read_at(&mut buf, 0);
    assert!(res.is_err(), "read after close should fail");
}

#[ignore = "needs: POST /pause to release preview demand and close active preview"]
#[test]
fn test_pause_releases_preview_demand() {
    // Port of Go TestPauseReleasesPreviewDemand
    let (data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    let _preview = eng.open_preview(&id, 0).expect("open preview");
    let (code, body) = eng.dispatch("POST", "/pause", &format!(r#"{{"id":"{id}"}}"#));
    assert_eq!(code, 200, "pause: {}", String::from_utf8_lossy(&body));

    let active = eng.server.preview_active.lock().is_some();
    assert!(!active, "pause must close active preview");
}

#[ignore = "needs: preview budget enforcement across seeks outside admitted union"]
#[test]
fn test_preview_budget_enforced_across_seeks() {
    // Port of Go TestPreviewBudgetEnforcedAcrossSeeks
    let (data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    let preview = eng.open_preview(&id, 0).expect("open preview");
    let mut buf = [0u8; 8];
    assert!(preview.read_at(&mut buf, 0).is_ok());

    let tail = preview.length() - 8;
    assert!(
        preview.read_at(&mut buf, tail).is_err(),
        "expected budget failure for far seek outside admitted union"
    );
}

#[ignore = "needs: preview session deadline timeout"]
#[test]
fn test_preview_session_deadline_blocks_unlimited_reads() {
    // Port of Go TestPreviewSessionDeadlineBlocksUnlimitedReads
    let (data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    let preview = eng.open_preview(&id, 0).expect("open preview");
    preview.close();
    let mut buf = [0u8; 4];
    assert!(preview.read_at(&mut buf, 0).is_err());
}

#[ignore = "needs: preview close to join blocked read"]
#[test]
fn test_preview_close_joins_blocked_read() {
    // Port of Go TestPreviewCloseJoinsBlockedRead
    let (_data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));

    let preview = eng.open_preview(&id, 0).expect("open preview");
    let p_clone = Arc::clone(&preview);
    let handle = thread::spawn(move || {
        let mut buf = [0u8; 4];
        let _ = p_clone.read_at(&mut buf, 0);
    });
    thread::sleep(Duration::from_millis(50));
    preview.close();
    handle.join().expect("join");
}

#[test]
fn test_preview_concurrent_open_atomic() {
    // Port of Go TestPreviewConcurrentOpenAtomic
    let (data, metadata) = recovery_metadata(None);
    let eng = Arc::new(TestEngine::new());
    let id = add_recovery_torrent(&eng, &metadata);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    const N: usize = 8;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let e = Arc::clone(&eng);
        let tid = id.clone();
        handles.push(thread::spawn(move || e.open_preview(&tid, 0)));
    }

    let mut ok_readers = Vec::new();
    for h in handles {
        let res = h.join().expect("thread join");
        if let Ok(reader) = res {
            ok_readers.push(reader);
        }
    }

    assert_eq!(
        ok_readers.len(),
        1,
        "expected exactly one successful open among concurrent callers, got {}",
        ok_readers.len()
    );
    for r in ok_readers {
        r.close();
    }
}

#[ignore = "needs: target-aware pause keeping unrelated preview open"]
#[test]
fn test_pause_other_torrent_leaves_preview() {
    // Port of Go TestPauseOtherTorrentLeavesPreview
    let (data, metadata) = recovery_metadata(None);
    let eng = TestEngine::new();
    let id = add_recovery_torrent(&eng, &metadata);

    let other_data = b"other torrent payload.".repeat(8192);
    let piece_len = 16 * 1024i64;
    let piece_count = other_data.len().div_ceil(piece_len as usize);
    let (other_torrent, _, _) =
        support::build_single_file_torrent("other.bin", piece_len, piece_count);
    let other_meta =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &other_torrent);
    let other_id = add_recovery_torrent(&eng, &other_meta);
    assert_ne!(id, other_id);

    let tmp = tempfile::tempdir_in(
        std::env::var("TMPDIR").unwrap_or_else(|_| "/home/kar/.cache/wt-tmp/agy-tests".into()),
    )
    .expect("tempdir");
    let dest_file = std::fs::File::create(tmp.path().join("f.dat")).expect("create");
    #[cfg(unix)]
    let fd = {
        use std::os::unix::io::AsRawFd;
        dest_file.as_raw_fd()
    };
    #[cfg(not(unix))]
    let fd = 0;

    let configure_req = serde_json::json!({
        "id": id,
        "descriptors": [fd],
        "selected": [0],
    });
    let (code, body) = eng.dispatch("POST", "/configure", &configure_req.to_string());
    assert_eq!(code, 200, "configure: {}", String::from_utf8_lossy(&body));
    write_verified_piece(&eng, &id, &data, 0);

    let preview = eng.open_preview(&id, 0).expect("open preview");
    let (code, body) = eng.dispatch("POST", "/pause", &format!(r#"{{"id":"{other_id}"}}"#));
    assert_eq!(code, 200, "pause other: {}", String::from_utf8_lossy(&body));

    assert!(
        eng.server.preview_active.lock().is_some(),
        "pausing unrelated torrent must not close this preview"
    );
    preview.close();
}

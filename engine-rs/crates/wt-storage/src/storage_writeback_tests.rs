use super::*;
use std::cell::RefCell;
use wt_types::FileInfo;

// Per-thread hooks at the actual pread/pwrite boundary, including the old
// locked fallbacks. Other tests and the background scheduler are unaffected.
type IoHook = Box<dyn Fn(bool)>;
thread_local! {
    static IO_HOOK: RefCell<Option<IoHook>> = RefCell::new(None);
}

pub(super) fn before_io(write: bool) {
    let hook = IO_HOOK.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook(write);
    }
}

fn fixture(cache_bytes: i64) -> (tempfile::TempDir, Arc<DocumentTorrentStorage>, File) {
    let dir = tempfile::tempdir().unwrap();
    let info = Info {
        name: "payload".into(),
        piece_length: 16,
        pieces: vec![0; 80],
        files: vec![FileInfo {
            length: 64,
            path: vec![],
            torrent_offset: 0,
            attr: String::new(),
            pieces_root: None,
        }],
        single_file: true,
        private: None,
        source: String::new(),
        meta_version: 0,
        file_tree_raw: None,
    };
    let storage = DocumentTorrentStorage::new(Some(info), InfoHash([8; 20]), dir.path());
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(dir.path().join("destination"))
        .unwrap();
    file.set_len(64).unwrap();
    storage
        .attach_descriptors(&[Some(file.as_raw_fd())], &[0])
        .unwrap();
    let mut inner = storage.inner.lock();
    // Drive scheduler passes explicitly to keep concurrency tests deterministic.
    inner.disk_flushing = true;
    inner.checkpoint_flush_scheduled = true;
    inner.max_cache_bytes = cache_bytes;
    drop(inner);
    (dir, storage, file)
}

#[test]
fn active_partial_is_saved_on_first_pass_without_sync() {
    let (_dir, storage, file) = fixture(1024);
    storage.piece_at(0).write_at(b"AAAA", 0).unwrap();
    assert!(!storage.run_disk_flush());
    let mut bytes = [0; 4];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"AAAA");
    let inner = storage.inner.lock();
    assert!(inner.saved_partial.get(&0).copied().unwrap_or(false));
    assert!(inner.pending_sync_fds.is_empty());
    assert!(!inner.cache_dirty[&0]);
}

#[test]
fn write_through_does_not_hold_storage_lock() {
    let (_dir, storage, file) = fixture(0);
    let checked = Arc::new(AtomicBool::new(false));
    let check = checked.clone();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |write| {
            assert!(write);
            check.store(st.inner.try_lock().is_some(), Ordering::SeqCst);
        }))
    });
    storage.piece_at(0).write_at(b"DATA", 0).unwrap();
    assert!(
        checked.load(Ordering::SeqCst),
        "write-through held storage mutex during disk I/O"
    );
    let mut bytes = [0; 4];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"DATA");
    assert!(storage.inner.lock().saved_partial[&0]);
}

#[test]
fn writing_evicted_completed_piece_does_not_read_disk() {
    let (_dir, storage, file) = fixture(16);
    file.write_all_at(b"abcdefghijklmnop", 0).unwrap();
    storage.inner.lock().completed.insert(0, true);
    let did_io = Arc::new(AtomicBool::new(false));
    let check = did_io.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            check.store(true, Ordering::SeqCst);
        }))
    });
    storage.piece_at(0).write_at(b"DATA", 4).unwrap();
    IO_HOOK.with(|slot| slot.borrow_mut().take());
    assert!(
        !did_io.load(Ordering::SeqCst),
        "receive path reloaded completed piece from disk"
    );
    let mut bytes = [0; 16];
    storage.piece_at(0).read_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"abcdDATAijklmnop");
}

#[test]
fn racing_flush_keeps_only_new_spans_dirty_and_saves_partial() {
    let (_dir, storage, file) = fixture(32);
    storage.piece_at(0).write_at(b"AAAA", 0).unwrap();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            st.piece_at(0).write_at(b"BBBB", 4).unwrap();
        }))
    });
    let (inner, result) = storage.flush_piece_unlocked(storage.inner.lock(), 0);
    assert!(!result.unwrap());
    assert_eq!(inner.dirty_spans[&0], vec![ByteRange { start: 4, end: 8 }]);
    assert!(inner.saved_partial.get(&0).copied().unwrap_or(false));
    drop(inner);
    // Previously saved ranges must not be rewritten by the next pass.
    file.write_all_at(b"KEEP", 0).unwrap();
    assert!(!storage.run_disk_flush());
    let mut bytes = [0; 8];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"KEEPBBBB");
}

#[test]
fn completion_retries_racing_writes_without_holding_lock() {
    let (_dir, storage, file) = fixture(32);
    storage.piece_at(0).write_at(b"AAAA", 0).unwrap();
    let checked = Arc::new(AtomicBool::new(false));
    let check = checked.clone();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            st.piece_at(0).write_at(b"BBBB", 4).unwrap();
            let st = st.clone();
            let check = check.clone();
            IO_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |_| {
                    check.store(st.inner.try_lock().is_some(), Ordering::SeqCst);
                }))
            });
        }))
    });
    storage.piece_at(0).mark_complete().unwrap();
    assert!(
        checked.load(Ordering::SeqCst),
        "completion retry held mutex during disk I/O"
    );
    let mut bytes = [0; 8];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"AAAABBBB");
    let inner = storage.inner.lock();
    assert!(inner.completed[&0]);
    assert!(!inner.saved_partial.contains_key(&0));
    assert!(!inner.pending_sync_fds.is_empty());
}

#[test]
fn eviction_flush_stays_in_ram_while_victim_is_still_receiving() {
    use std::sync::mpsc;
    let (_dir, storage, file) = fixture(16);
    storage.piece_at(0).write_at(b"AAAAAAAA", 0).unwrap();
    let (go_tx, go_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let st = storage.clone();
    let done = finished.clone();
    let writer = std::thread::spawn(move || {
        go_rx.recv().unwrap();
        entered_tx.send(()).unwrap();
        st.piece_at(0).write_at(b"BBBBBBBB", 8).unwrap();
        done.store(true, Ordering::SeqCst);
    });
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            // One blocked pass is enough: later retries must proceed.
            let _ = IO_HOOK.with(|slot| slot.borrow_mut().take());
            go_tx.send(()).unwrap();
            entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            // The in-flight receive has to wait out this eviction. If it
            // landed in the slot, the flush would stay dirty and the new
            // piece would write straight through.
            std::thread::sleep(Duration::from_millis(80));
            assert!(
                !finished.load(Ordering::SeqCst),
                "receive into the victim finished during its eviction flush"
            );
        }))
    });
    storage.piece_at(1).write_at(b"CCCCCCCC", 0).unwrap();
    let mut got = [0u8; 8];
    storage.piece_at(1).read_at(&mut got, 0).unwrap();
    assert_eq!(&got, b"CCCCCCCC");
    writer.join().unwrap();
    let mut bytes = [0; 16];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes[..8], b"AAAAAAAA");
}

#[test]
fn failed_victim_flush_rechecks_slot_created_by_another_writer() {
    let (dir, storage, _file) = fixture(16);
    storage.piece_at(0).write_at(b"AAAA", 0).unwrap();
    let read_only = File::open(dir.path().join("destination")).unwrap();
    storage.replace_file_handle(0, read_only).unwrap();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            // A failed verification can drop the victim while its flush is
            // unlocked. Another peer can then create our target's cache slot.
            st.piece_at(0).mark_not_complete().unwrap();
            st.piece_at(1).write_at(b"BBBB", 4).unwrap();
        }))
    });
    storage.piece_at(1).write_at(b"DATA", 0).unwrap();
    let mut bytes = [0; 8];
    storage.piece_at(1).read_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"DATABBBB");
}

#[test]
fn slow_write_through_allows_other_peer_to_write_cached_piece() {
    use std::sync::mpsc;
    let (_dir, storage, _file) = fixture(16);
    storage.piece_at(0).write_at(b"AAAA", 0).unwrap();
    // Simulate the only cache slot already being written back.
    storage.inner.lock().flushing.insert(0);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let st = storage.clone();
    let writer = std::thread::spawn(move || {
        IO_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }))
        });
        st.piece_at(1).write_at(b"DATA", 0)
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let st = storage.clone();
    let other = std::thread::spawn(move || {
        done_tx.send(st.piece_at(0).write_at(b"BBBB", 4)).unwrap();
    });
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    writer.join().unwrap().unwrap();
    other.join().unwrap();
    assert_eq!(
        result
            .expect("peer blocked by unrelated slow disk write")
            .unwrap(),
        4
    );
    storage.inner.lock().flushing.remove(&0);
}

#[test]
fn write_through_retries_replaced_files_without_holding_lock() {
    let (dir, storage, old) = fixture(0);
    let replacement = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(dir.path().join("replacement"))
        .unwrap();
    replacement.set_len(64).unwrap();
    let file = replacement.try_clone().unwrap();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            st.replace_file_handle(0, file.try_clone().unwrap())
                .unwrap();
        }))
    });
    storage.piece_at(0).write_at(b"DATA", 0).unwrap();
    let mut bytes = [0; 4];
    replacement.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"DATA");
    old.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"DATA");
    assert!(storage.inner.lock().flushing.is_empty());
}

#[test]
fn saved_partial_survives_repeated_eviction_without_zero_overwrites() {
    let (_dir, storage, file) = fixture(16);
    file.write_all_at(b"abcdefghijklmnop", 0).unwrap();
    let piece = storage.piece_at(0);
    piece.write_at(b"AAAA", 0).unwrap();
    assert!(!storage.run_disk_flush());
    storage.inner.lock().disk_flushing = true;
    storage.piece_at(1).write_at(b"XXXX", 0).unwrap();
    piece.write_at(b"BBBB", 4).unwrap();
    assert!(!storage.run_disk_flush());
    storage.inner.lock().disk_flushing = true;
    storage.piece_at(1).write_at(b"YYYY", 4).unwrap();
    piece.write_at(b"CCCC", 8).unwrap();
    let mut bytes = [0; 16];
    piece.read_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"AAAABBBBCCCCmnop");
    assert!(!storage.run_disk_flush());
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"AAAABBBBCCCCmnop");
}

#[test]
fn verified_disk_preview_does_not_hold_storage_lock() {
    let (_dir, storage, file) = fixture(16);
    file.write_all_at(b"abcdefghijklmnop", 0).unwrap();
    storage.inner.lock().completed.insert(0, true);
    let checked = Arc::new(AtomicBool::new(false));
    let check = checked.clone();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |write| {
            assert!(!write);
            check.store(st.inner.try_lock().is_some(), Ordering::SeqCst);
        }));
    });
    let mut bytes = [0; 8];
    let (n, ok) = storage.read_verified_preview(&mut bytes, 4);
    assert!(ok);
    assert_eq!(n, 8);
    assert_eq!(&bytes, b"efghijkl");
    assert!(
        checked.load(Ordering::SeqCst),
        "preview held storage mutex during disk read"
    );
}

#[test]
fn verified_disk_preview_partial_fill_does_not_hold_storage_lock() {
    let (_dir, storage, file) = fixture(16);
    file.write_all_at(b"abcdefghijklmnop", 0).unwrap();
    storage.piece_at(0).write_at(b"DATA", 4).unwrap();
    storage.inner.lock().completed.insert(0, true);
    let checked = Arc::new(AtomicBool::new(false));
    let check = checked.clone();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            check.store(st.inner.try_lock().is_some(), Ordering::SeqCst);
        }));
    });
    let mut bytes = [0; 16];
    let (n, ok) = storage.read_verified_preview(&mut bytes, 0);
    assert!(ok);
    assert_eq!(n, 16);
    assert_eq!(&bytes, b"abcdDATAijklmnop");
    assert!(
        checked.load(Ordering::SeqCst),
        "preview partial fill held mutex during disk I/O"
    );
}

#[test]
fn scheduled_checkpoint_metadata_does_not_hold_storage_lock() {
    let (_dir, storage, _file) = fixture(16);
    storage.inner.lock().checkpoint_dirty = true;
    let checked = Arc::new(AtomicBool::new(false));
    let check = checked.clone();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |write| {
            assert!(!write);
            check.store(st.inner.try_lock().is_some(), Ordering::SeqCst);
        }));
    });
    assert!(storage.drain_sync_and_checkpoint_with(false));
    assert!(
        checked.load(Ordering::SeqCst),
        "checkpoint held storage mutex during file metadata I/O"
    );
}

#[test]
fn checkpoint_retries_if_file_changes_during_unlocked_metadata() {
    let (dir, storage, _file) = fixture(16);
    storage.inner.lock().checkpoint_dirty = true;
    let replacement = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(dir.path().join("replacement-metadata"))
        .unwrap();
    replacement.set_len(64).unwrap();
    let file = replacement.try_clone().unwrap();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |_| {
            st.replace_file_handle(0, file.try_clone().unwrap())
                .unwrap();
        }));
    });
    assert!(!storage.drain_sync_and_checkpoint_with(false));
    assert!(storage.inner.lock().checkpoint_dirty);
    let path = checkpoint_path(dir.path(), &storage.info_hash.to_hex());
    assert!(!path.exists(), "stale file identities were published");
    assert!(storage.drain_sync_and_checkpoint_with(false));
    let cp = read_piece_checkpoint(&path).unwrap();
    let (expected, _) = identity_from_file(&replacement, true, 64, "payload".into());
    assert_eq!(cp.files[0].ino, expected.ino);
    assert_eq!(cp.files[0].dev, expected.dev);
    assert!(!storage.inner.lock().checkpoint_dirty);
}

#[test]
fn slow_disk_preview_allows_another_peer_to_receive() {
    use std::sync::mpsc;
    let (_dir, storage, file) = fixture(16);
    file.write_all_at(b"abcdefghijklmnop", 0).unwrap();
    storage.inner.lock().completed.insert(0, true);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let st = storage.clone();
    let preview = std::thread::spawn(move || {
        IO_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }));
        });
        let mut bytes = [0; 4];
        let result = st.read_verified_preview(&mut bytes, 0);
        (result, bytes)
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let st = storage.clone();
    let receiver = std::thread::spawn(move || {
        done_tx.send(st.piece_at(1).write_at(b"PEER", 0)).unwrap();
    });
    let received = done_rx.recv_timeout(Duration::from_millis(500));
    release_tx.send(()).unwrap();
    receiver.join().unwrap();
    let (result, bytes) = preview.join().unwrap();
    assert_eq!(result, (4, true));
    assert_eq!(&bytes, b"abcd");
    assert!(received.is_ok(), "disk preview blocked the receive path");
    assert_eq!(received.unwrap().unwrap(), 4);
}

#[test]
fn completion_during_sync_is_not_published_before_its_own_sync() {
    let (dir, storage, _file) = fixture(64);
    storage
        .piece_at(0)
        .write_at(b"AAAAAAAAAAAAAAAA", 0)
        .unwrap();
    storage.piece_at(0).mark_complete().unwrap();
    let st = storage.clone();
    storage.set_file_sync_hook(Some(Arc::new(move |_| {
        st.piece_at(1).write_at(b"BBBBBBBBBBBBBBBB", 0).unwrap();
        st.piece_at(1).mark_complete().unwrap();
        Ok(())
    })));
    assert!(storage.drain_sync_and_checkpoint_with(false));
    storage.set_file_sync_hook(None);
    let path = checkpoint_path(dir.path(), &storage.info_hash.to_hex());
    let cp = read_piece_checkpoint(&path).unwrap();
    assert!(cp.complete[0]);
    assert!(
        !cp.complete[1],
        "checkpoint included a piece completed after this pass took its data-sync snapshot"
    );
    assert!(storage.inner.lock().checkpoint_dirty);
    assert!(!storage.inner.lock().pending_sync_fds.is_empty());
    assert!(storage.drain_sync_and_checkpoint_with(false));
    assert!(read_piece_checkpoint(&path).unwrap().complete[1]);
}

#[test]
fn partial_fill_retries_when_backing_file_changes_during_read() {
    let (dir, storage, file) = fixture(16);
    file.write_all_at(b"abcdefghijklmnop", 0).unwrap();
    storage.piece_at(0).write_at(b"DATA", 4).unwrap();
    storage.run_disk_flush();
    let replacement = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(dir.path().join("replacement-read"))
        .unwrap();
    replacement.set_len(64).unwrap();
    replacement.write_all_at(b"ABCDEFGHIJKLMNOP", 0).unwrap();
    let st = storage.clone();
    IO_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |write| {
            assert!(!write);
            st.replace_file_handle(0, replacement.try_clone().unwrap())
                .unwrap();
        }));
    });
    let mut bytes = [0; 16];
    assert!(
        matches!(
            storage.piece_at(0).read_at(&mut bytes, 0),
            Err(StorageError::Unavailable)
        ),
        "partial fill must not install old-file data in the current cache"
    );
    storage.piece_at(0).read_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"ABCDDATAIJKLMNOP");
}

#[test]
fn in_memory_write_only_into_an_existing_slot() {
    // Room for one 16-byte piece.
    let (_dir, storage, _file) = fixture(16);
    // No slot yet: creating one may write a victim back first.
    assert!(storage.piece_at(0).write_at_in_memory(b"AAAA", 0).is_none());
    storage.piece_at(0).write_at(b"AAAA", 0).unwrap();
    assert_eq!(storage.piece_at(0).write_at_in_memory(b"BBBB", 4).unwrap().unwrap(), 4);
    let mut bytes = [0; 8];
    storage.piece_at(0).read_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"AAAABBBB");
    // Piece 1 needs piece 0 evicted, which writes it to disk.
    assert!(storage.piece_at(1).write_at_in_memory(b"CCCC", 0).is_none());
    // A slot being evicted waits for its write-back; nothing is written.
    storage.inner.lock().evicting.insert(0);
    assert!(storage.piece_at(0).write_at_in_memory(b"DDDD", 0).is_none());
    storage.inner.lock().evicting.remove(&0);
    storage.piece_at(0).read_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"AAAABBBB");
}

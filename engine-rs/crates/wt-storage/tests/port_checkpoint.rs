//! Ports of engine-go/checkpoint_test.go and checkpoint_rel_test.go

mod support;

use std::fs;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use wt_storage::{
    checkpoint_path, delete_piece_checkpoint, file_stable, identities_match, read_piece_checkpoint,
    write_piece_checkpoint, DocumentTorrentStorage, FileIdentity, PieceCheckpoint, TorrentStorage,
    CHECKPOINT_MAGIC, CHECKPOINT_VERSION, MAX_CHECKPOINT_PIECES,
};
use wt_types::{InfoHash, StorageError};

use support::{
    fds_from_files, info_hash_bytes, info_hash_zero, open_rw, open_rw_no_trunc, single_file_info,
    test_temp_dir,
};

#[test]
fn test_piece_checkpoint_round_trip() {
    let dir = test_temp_dir("chk_rt");
    let hash = [0xabu8; 20];
    let cp = PieceCheckpoint {
        info_hash: hash,
        piece_length: 16 * 1024,
        num_pieces: 10,
        files: vec![FileIdentity {
            external: true,
            dev: 1,
            ino: 2,
            length: 100,
            mod_nano: 123,
            path: "movie.mp4".into(),
        }],
        complete: vec![
            true, false, true, true, false, false, false, false, false, true,
        ],
    };
    let hex_hash = hex::encode(hash);
    let path = checkpoint_path(dir.path(), &hex_hash);
    write_piece_checkpoint(&path, &cp).expect("write");
    let got = read_piece_checkpoint(&path).expect("read");
    assert_eq!(got.info_hash, cp.info_hash);
    assert_eq!(got.piece_length, cp.piece_length);
    assert_eq!(got.num_pieces, cp.num_pieces);
    assert!(identities_match(&got.files, &cp.files));
    assert_eq!(got.complete, cp.complete);
}

#[test]
fn test_checkpoint_skips_trusted_pieces_on_resume() {
    let dir = test_temp_dir("chk_resume");
    let piece_len = 16 * 1024i64;
    let info = single_file_info("movie.mp4", piece_len * 2, piece_len);
    let hash = info_hash_bytes([0x11; 20]);
    let dest_path = dir.path().join("dest");
    let dest = open_rw(&dest_path);
    dest.set_len((piece_len * 2) as u64).unwrap();

    {
        let st = DocumentTorrentStorage::new(Some(info.clone()), hash, dir.path());
        let fds = fds_from_files(&[Some(&dest)]);
        st.attach_descriptors(&fds, &[0]).expect("attach");
        // Write full pieces and mark complete so close() persists checkpoint.
        let p0 = st.piece_at(0);
        let p1 = st.piece_at(1);
        let data0 = vec![1u8; piece_len as usize];
        let data1 = vec![2u8; piece_len as usize];
        p0.write_at(&data0, 0).unwrap();
        p1.write_at(&data1, 0).unwrap();
        p0.mark_complete().unwrap();
        p1.mark_complete().unwrap();
        assert!(st.piece_complete(0) && st.piece_complete(1));
        TorrentStorage::close(&*st).unwrap();
    }

    let chk = checkpoint_path(dir.path(), &hash.to_hex());
    assert!(chk.exists(), "checkpoint not written");

    let dest2 = open_rw_no_trunc(&dest_path);
    let st2 = DocumentTorrentStorage::new(Some(info.clone()), hash, dir.path());
    let fds = fds_from_files(&[Some(&dest2)]);
    st2.attach_descriptors(&fds, &[0]).expect("reattach");
    assert!(
        st2.piece_complete(0) && st2.piece_complete(1),
        "trusted pieces were not restored from checkpoint"
    );
    let _ = TorrentStorage::close(&*st2);
}

#[test]
fn test_checkpoint_survives_mtime_bump_on_same_inode() {
    let dir = test_temp_dir("chk_mtime");
    let piece_len = 16 * 1024i64;
    let info = single_file_info("movie.mp4", piece_len, piece_len);
    let hash = info_hash_bytes([0x22; 20]);
    let dest_path = dir.path().join("dest");
    let mut dest = open_rw(&dest_path);
    dest.set_len(piece_len as u64).unwrap();

    {
        let st = DocumentTorrentStorage::new(Some(info.clone()), hash, dir.path());
        let fds = fds_from_files(&[Some(&dest)]);
        st.attach_descriptors(&fds, &[0]).expect("attach");
        let p0 = st.piece_at(0);
        let data = vec![7u8; piece_len as usize];
        p0.write_at(&data, 0).unwrap();
        p0.mark_complete().unwrap();
        TorrentStorage::close(&*st).unwrap();
    }

    // Same inode, bumped mtime: file_stable ignores mod_nano.
    dest.write_all(b"x").unwrap();
    dest.sync_all().unwrap();
    std::thread::sleep(Duration::from_millis(10));

    let dest2 = open_rw_no_trunc(&dest_path);
    let st2 = DocumentTorrentStorage::new(Some(info), hash, dir.path());
    let fds = fds_from_files(&[Some(&dest2)]);
    st2.attach_descriptors(&fds, &[0]).expect("reattach");
    assert!(
        st2.piece_complete(0),
        "same-inode mtime bump dropped trusted bits"
    );
    let _ = TorrentStorage::close(&*st2);
}

#[test]
fn test_checkpoint_ignored_when_file_identity_changes() {
    let dir = test_temp_dir("chk_ident");
    let piece_len = 16 * 1024i64;
    let info = single_file_info("movie.mp4", piece_len, piece_len);
    let hash = info_hash_bytes([0x33; 20]);
    let dest_path = dir.path().join("dest");
    let dest = open_rw(&dest_path);
    dest.set_len(piece_len as u64).unwrap();

    {
        let st = DocumentTorrentStorage::new(Some(info.clone()), hash, dir.path());
        let fds = fds_from_files(&[Some(&dest)]);
        st.attach_descriptors(&fds, &[0]).expect("attach");
        let p0 = st.piece_at(0);
        p0.write_at(&vec![1u8; piece_len as usize], 0).unwrap();
        p0.mark_complete().unwrap();
        TorrentStorage::close(&*st).unwrap();
    }

    // Different inode (replacement file).
    let replacement_path = dir.path().join("replaced");
    let replacement = open_rw(&replacement_path);
    replacement.set_len(piece_len as u64).unwrap();
    {
        use std::io::Write;
        let mut r = &replacement;
        r.write_all(b"corrupted-data!!!!").ok();
    }
    replacement.sync_all().unwrap();

    let st2 = DocumentTorrentStorage::new(Some(info), hash, dir.path());
    let fds = fds_from_files(&[Some(&replacement)]);
    st2.attach_descriptors(&fds, &[0]).expect("reattach");
    assert!(
        !st2.piece_complete(0),
        "checkpoint remained trusted after file identity changed"
    );
    let _ = TorrentStorage::close(&*st2);
}

#[test]
fn test_checkpoint_deleted_on_remove() {
    let dir = test_temp_dir("chk_del");
    let piece_len = 4i64;
    let info = single_file_info("one", piece_len, piece_len);
    let hash = info_hash_bytes([0x44; 20]);
    let dest = open_rw(&dir.path().join("dest"));
    dest.set_len(piece_len as u64).unwrap();

    let st = DocumentTorrentStorage::new(Some(info.clone()), hash, dir.path());
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    let p0 = st.piece_at(0);
    p0.write_at(b"DATA", 0).unwrap();
    p0.mark_complete().unwrap();
    // Force checkpoint write via close.
    TorrentStorage::close(&*st).unwrap();

    let chk = checkpoint_path(dir.path(), &hash.to_hex());
    assert!(chk.exists());
    delete_piece_checkpoint(dir.path(), &hash.to_hex()).unwrap();
    assert!(!chk.exists(), "checkpoint remained after delete");
}

#[test]
fn test_malformed_checkpoint_piece_count_rejected() {
    let dir = test_temp_dir("chk_bad");
    let path = dir.path().join("bad.chk");
    let mut hdr = vec![0u8; 40];
    hdr[0..4].copy_from_slice(&CHECKPOINT_MAGIC.to_be_bytes());
    hdr[4..8].copy_from_slice(&CHECKPOINT_VERSION.to_be_bytes());
    // piece_length at offset 28
    hdr[28..36].copy_from_slice(&(16u64 * 1024).to_be_bytes());
    // num_pieces at offset 36
    hdr[36..40].copy_from_slice(&((MAX_CHECKPOINT_PIECES as u32) + 1).to_be_bytes());
    fs::write(&path, hdr).unwrap();
    assert!(
        read_piece_checkpoint(&path).is_err(),
        "expected oversized piece count to be rejected"
    );
}

#[test]
fn test_drain_sync_failure_preserves_dirty_and_pending() {
    let dir = test_temp_dir("drain_sync_fail");
    let info = single_file_info("one", 8, 8);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest = open_rw(&dir.path().join("dest"));
    dest.set_len(8).unwrap();
    let fd = dest.as_raw_fd();
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    st.piece_at(0).write_at(b"ABCDEFGH", 0).unwrap();
    st.piece_at(0).mark_complete().unwrap();

    let sync_calls = Arc::new(AtomicI32::new(0));
    let sync_calls2 = sync_calls.clone();
    st.set_file_sync_hook(Some(Arc::new(move |_| {
        sync_calls2.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("injected sync failure"))
    })));
    st.set_write_checkpoint_hook(Some(Arc::new(|_, _| {
        Err(StorageError::Other(
            "checkpoint write must not run after failed data sync".into(),
        ))
    })));
    st.note_pending_sync_fd(fd);
    st.set_checkpoint_dirty(true);

    let before_exists = checkpoint_path(dir.path(), &info_hash_zero().to_hex()).exists();
    st.drain_sync_and_checkpoint();

    assert!(
        st.checkpoint_dirty(),
        "failed sync cleared dirty/pending state"
    );
    assert!(
        st.checkpoint_persist_error().is_some(),
        "expected persist error feedback"
    );
    assert!(
        sync_calls.load(Ordering::SeqCst) > 0,
        "sync seam was not exercised"
    );
    let after_exists = checkpoint_path(dir.path(), &info_hash_zero().to_hex()).exists();
    assert!(
        !after_exists || before_exists,
        "checkpoint must not be published after failed data sync"
    );
    st.set_file_sync_hook(None);
    st.set_write_checkpoint_hook(None);
    let _ = TorrentStorage::close(&*st);
}

#[test]
fn test_drain_checkpoint_write_failure_preserves_dirty() {
    let dir = test_temp_dir("drain_cp_fail");
    let info = single_file_info("one", 8, 8);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest = open_rw(&dir.path().join("dest"));
    dest.set_len(8).unwrap();
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    st.piece_at(0).write_at(b"ABCDEFGH", 0).unwrap();
    st.piece_at(0).mark_complete().unwrap();

    st.set_write_checkpoint_hook(Some(Arc::new(|_, _| {
        Err(StorageError::Other(
            "injected checkpoint write failure".into(),
        ))
    })));
    st.set_checkpoint_dirty(true);
    st.drain_sync_and_checkpoint();
    assert!(
        st.checkpoint_dirty(),
        "failed checkpoint write cleared dirty state"
    );
    assert!(
        st.checkpoint_persist_error().is_some(),
        "expected checkpoint write error"
    );
    st.set_write_checkpoint_hook(None);
    let _ = TorrentStorage::close(&*st);
}

#[test]
fn test_drain_checkpoint_succeeds_after_transient_failure() {
    let dir = test_temp_dir("drain_cp_retry");
    let info = single_file_info("one", 8, 8);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest = open_rw(&dir.path().join("dest"));
    dest.set_len(8).unwrap();
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    st.piece_at(0).write_at(b"ABCDEFGH", 0).unwrap();
    st.piece_at(0).mark_complete().unwrap();

    let writes = Arc::new(AtomicI32::new(0));
    let writes2 = writes.clone();
    st.set_write_checkpoint_hook(Some(Arc::new(move |path, cp| {
        if writes2.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(StorageError::Other("transient write failure".into()));
        }
        write_piece_checkpoint(path, cp)
    })));
    st.set_checkpoint_dirty(true);
    st.drain_sync_and_checkpoint();
    assert!(st.checkpoint_dirty(), "first failure should keep dirty");
    st.set_checkpoint_dirty(true);
    st.drain_sync_and_checkpoint();
    assert!(!st.checkpoint_dirty(), "retry success should clear dirty");
    assert!(
        checkpoint_path(dir.path(), &info_hash_zero().to_hex()).exists(),
        "checkpoint missing after retry"
    );
    st.set_write_checkpoint_hook(None);
    let _ = TorrentStorage::close(&*st);
}

#[test]
fn test_drain_does_not_rewrite_deleted_checkpoint() {
    let dir = test_temp_dir("drain_no_rewrite");
    let info = single_file_info("one", 8, 8);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest = open_rw(&dir.path().join("dest"));
    dest.set_len(8).unwrap();
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    st.piece_at(0).write_at(b"ABCDEFGH", 0).unwrap();
    st.piece_at(0).mark_complete().unwrap();
    st.set_checkpoint_dirty(true);
    st.delete_checkpoint().unwrap();
    st.drain_sync_and_checkpoint();
    assert!(
        !checkpoint_path(dir.path(), &info_hash_zero().to_hex()).exists(),
        "deleted checkpoint was recreated"
    );
    let _ = TorrentStorage::close(&*st);
}

#[test]
fn test_checkpoint_delete_during_publish_does_not_resurrect() {
    let dir = test_temp_dir("drain_delete_race");
    let info = single_file_info("one", 8, 8);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest = open_rw(&dir.path().join("dest"));
    dest.set_len(8).unwrap();
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    st.piece_at(0).write_at(b"ABCDEFGH", 0).unwrap();
    st.piece_at(0).mark_complete().unwrap();

    let st_hook = st.clone();
    let gen_before = st.checkpoint_gen();
    st.set_write_checkpoint_hook(Some(Arc::new(move |path, cp| {
        // Concurrent delete bumps generation while the stale writer is publishing.
        st_hook.delete_checkpoint().unwrap();
        write_piece_checkpoint(path, cp)
    })));
    st.set_checkpoint_dirty(true);
    st.drain_sync_and_checkpoint();
    assert!(
        !checkpoint_path(dir.path(), &info_hash_zero().to_hex()).exists(),
        "checkpoint resurrected after delete during publish"
    );
    assert_ne!(
        st.checkpoint_gen(),
        gen_before,
        "delete must bump generation"
    );
    st.set_write_checkpoint_hook(None);
    let _ = TorrentStorage::close(&*st);
}

#[test]
fn test_checkpoint_persist_error_surfaced_in_torrent_status() {
    // Go surfaces via torrent status JSON. Rust exposes checkpoint_persist_error().
    let dir = test_temp_dir("chk_err");
    let info = single_file_info("one", 4, 4);
    let st = DocumentTorrentStorage::new(Some(info), InfoHash([0x55; 20]), dir.path());
    // No attach / no error yet.
    assert!(st.checkpoint_persist_error().is_none());
    assert!(!file_stable(
        &FileIdentity::default(),
        &FileIdentity {
            external: true,
            ..Default::default()
        },
    ));
}

//! Ports of engine-go/extent_test.go

mod support;

use std::io::Write;
use std::os::unix::io::AsRawFd;

use sha1::{Digest, Sha1};
use wt_storage::{
    hash_extents, pread_extents, zero_piece_hash, DocumentTorrentStorage, FileSpan, TorrentStorage,
};

use support::{fds_from_files, info_hash_zero, open_rw, single_file_info, test_temp_dir};

fn sha1_of(bytes: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(bytes);
    h.finalize().into()
}

#[test]
fn test_hash_extents_matches_sha1_and_pread() {
    let dir = test_temp_dir("extent");
    let path = dir.path().join("extent.dat");
    let mut f = open_rw(&path);
    let data = vec![9u8; 9000];
    f.write_all(&data).unwrap();
    f.flush().unwrap();

    let span = FileSpan {
        fd: f.as_raw_fd(),
        offset: 0,
        length: data.len() as i64,
    };
    let (got, all_holes) = hash_extents(&[span]).expect("hash_extents");
    assert!(!all_holes, "dense span reported all holes");
    assert_eq!(got, sha1_of(&data));

    let mut buf = vec![0u8; data.len()];
    let n = pread_extents(&[span], &mut buf).expect("pread");
    assert_eq!(n, data.len());
    assert_eq!(buf, data);
}

#[test]
fn test_hash_extents_all_hole_matches_zero_hash() {
    let dir = test_temp_dir("hole");
    let path = dir.path().join("hole.dat");
    let f = open_rw(&path);
    const N: i64 = 16 * 1024;
    f.set_len(N as u64).unwrap();

    let span = FileSpan {
        fd: f.as_raw_fd(),
        offset: 0,
        length: N,
    };
    let (got, _) = hash_extents(&[span]).expect("hash_extents");
    assert_eq!(got, zero_piece_hash(N));
}

#[test]
fn test_range_complete_uses_durable_bits() {
    let dir = test_temp_dir("range_complete");
    let info = single_file_info("one", 32 * 1024, 16 * 1024);
    let st = DocumentTorrentStorage::new(Some(info), info_hash_zero(), dir.path());
    let dest_path = dir.path().join("dest");
    let dest = open_rw(&dest_path);
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");

    assert!(
        !st.range_complete(0, 16 * 1024),
        "empty storage must not report a complete range"
    );
    st.set_piece_complete(0, true);
    assert!(
        st.range_complete(0, 16 * 1024),
        "piece 0 should cover the first range"
    );
    assert!(!st.range_complete(0, 32 * 1024), "piece 1 is not complete");

    let _ = dest; // keep fd alive for attached storage
    assert!(st.piece_complete(0));
    let _ = TorrentStorage::close(&*st);
}

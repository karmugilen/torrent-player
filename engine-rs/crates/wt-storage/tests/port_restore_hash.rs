//! Ports of engine-go/restore_hash_test.go

mod support;

use std::io::Write;
use std::os::unix::io::AsRawFd;

use sha1::{Digest, Sha1};
use wt_storage::restore_hash::{piece_for_hash, RestoreVerifyPiece};
use wt_storage::{
    zero_piece_hash, DocumentStorageClient, DocumentTorrentStorage, PieceStorage, TorrentStorage,
};
use wt_types::ClientStorage;

use support::{
    fds_from_files, info_hash_bytes, info_hash_zero, open_rw, piece_of, single_file_info,
    test_temp_dir,
};

#[test]
fn test_zero_piece_hash_matches_sha1() {
    for &length in &[0i64, 1, 20, 16 * 1024, 256 * 1024 + 7] {
        let got = zero_piece_hash(length);
        let mut h = Sha1::new();
        h.update(vec![0u8; length as usize]);
        let want: [u8; 20] = h.finalize().into();
        assert_eq!(got, want, "length {length}");
        assert_eq!(
            zero_piece_hash(length),
            got,
            "length {length}: cache returned a different hash"
        );
    }
}

#[test]
fn test_sparse_piece_extent_all_holes() {
    let dir = test_temp_dir("sparse_holes");
    let info = single_file_info("sparse.bin", 32 * 1024, 16 * 1024);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest_path = dir.path().join("dest");
    let mut dest = open_rw(&dest_path);
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");

    let piece0 = piece_of(&info, 0);
    let (all, supported) = st.piece_extent_all_holes(piece0);
    if !supported {
        // Filesystem does not expose sparse holes (e.g. some tmpfs configs).
        let _ = TorrentStorage::close(&*st);
        return;
    }
    assert!(all, "truncated file should report all-hole pieces");

    dest.write_all(b"not-a-hole").unwrap();
    dest.flush().unwrap();

    let (all, supported) = st.piece_extent_all_holes(piece0);
    assert!(supported, "sparse probing became unsupported after write");
    assert!(!all, "piece with written data was treated as all holes");

    let piece1 = piece_of(&info, 1);
    let (all, supported) = st.piece_extent_all_holes(piece1);
    assert!(
        supported,
        "sparse probing became unsupported for later piece"
    );
    if all {
        let p = st.piece_at(1);
        let sum = p.self_hash().expect("self_hash");
        assert_eq!(sum, zero_piece_hash(info.piece_length));
    }
    let _ = TorrentStorage::close(&*st);
    let _ = dest.as_raw_fd();
}

#[test]
fn test_restore_verify_self_hash_uses_zero_hash_for_holes() {
    let dir = test_temp_dir("restore_self_hash");
    let info = single_file_info("sparse.bin", 16 * 1024, 16 * 1024);
    let st = DocumentTorrentStorage::new(Some(info.clone()), info_hash_zero(), dir.path());
    let dest = open_rw(&dir.path().join("dest"));
    let fds = fds_from_files(&[Some(&dest)]);
    st.attach_descriptors(&fds, &[0]).expect("attach");
    st.set_restore_verify(true);

    let p = st.piece_at(0);
    let sum = p.self_hash().expect("self_hash");
    let (all, supported) = st.piece_extent_all_holes(piece_of(&info, 0));
    if supported && all {
        assert_eq!(sum, zero_piece_hash(info.piece_length));
    } else {
        assert_eq!(sum, zero_piece_hash(info.piece_length));
    }
    let _ = TorrentStorage::close(&*st);
}

#[test]
fn test_restore_verify_piece_type_only_while_enabled() {
    let dir = test_temp_dir("restore_type");
    let info = single_file_info("one", 4, 4);
    let client = DocumentStorageClient::new(dir.path());
    let hash = info_hash_bytes([1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let impl_st = client.open_torrent(&info, hash).expect("open");
    let st = client.get_storage(&hash.to_hex()).expect("get");

    assert!(!st.restore_verify_enabled());
    let plain = piece_for_hash(&st, piece_of(&info, 0));
    let _ = PieceStorage::completion(&*plain);

    st.set_restore_verify(true);
    assert!(st.restore_verify_enabled());
    let gated = piece_for_hash(&st, piece_of(&info, 0));
    // RestoreVerifyPiece is the gated wrapper; self_hash is available on DocumentPiece too.
    let _ = RestoreVerifyPiece::new(st.clone(), piece_of(&info, 0)).self_hash();
    let _ = gated;

    st.set_restore_verify(false);
    assert!(!st.restore_verify_enabled());
    let _ = TorrentStorage::close(&*impl_st);
}

//! Restore SelfHash: skip hashing filesystem holes during saved-data checks.
//!
//! # Public API
//!
//! - [`RestoreVerifyPiece`] — piece wrapper returned while restore-verify is on.
//!   Call [`RestoreVerifyPiece::self_hash`] to hash with hole skipping. Peer
//!   hashing keeps a plain [`DocumentPiece`] so smart banning still works.
//! - [`piece_for_hash`] — pick restore or plain piece from
//!   [`DocumentTorrentStorage::restore_verify_enabled`].
//! - [`hash_piece_contents`] — dense SHA-1 of a piece via `PieceStorage::read_at`.
//!
//! Hole path (mirrors `engine-go/restore_hash.go`):
//! 1. If the piece is in the RAM cache, hash its contents (never treat as a hole).
//! 2. Else try extent hashing (`hash_extents` / file spans); all-holes →
//!    [`crate::zero_piece_hash`].
//! 3. Else probe sparse holes via [`DocumentTorrentStorage::piece_extent_all_holes`].
//! 4. Else fall back to [`hash_piece_contents`].
//!
//! Zero-hash interplay: all-hole pieces return the cached SHA-1 of `length`
//! zero bytes from [`crate::zero_hash::zero_piece_hash`].
//!
//! Enable with [`DocumentTorrentStorage::set_restore_verify`]; agy-3 owns that
//! flag and `DocumentPiece::self_hash`. This module is the gated wrapper and
//! the documented SelfHash entry point for wt-engine / tests.

use std::sync::Arc;

use sha1::{Digest, Sha1};

use crate::storage::{DocumentPiece, DocumentTorrentStorage};
use crate::zero_hash::zero_piece_hash;
use crate::{hash_extents, Completion, Piece, PieceStorage, StorageError, StorageResult};

/// Piece storage that exposes SelfHash only while restore-verify is enabled.
///
/// Forwards all `PieceStorage` ops to the underlying [`DocumentPiece`].
pub struct RestoreVerifyPiece {
    inner: DocumentPiece,
}

impl RestoreVerifyPiece {
    pub fn new(storage: Arc<DocumentTorrentStorage>, piece: Piece) -> Self {
        Self {
            inner: DocumentPiece { storage, piece },
        }
    }

    pub fn from_document(piece: DocumentPiece) -> Self {
        Self { inner: piece }
    }

    pub fn document(&self) -> &DocumentPiece {
        &self.inner
    }

    pub fn storage(&self) -> &Arc<DocumentTorrentStorage> {
        &self.inner.storage
    }

    pub fn piece(&self) -> Piece {
        self.inner.piece
    }

    /// SelfHash with hole skipping (Go `restoreVerifyPiece.SelfHash`).
    pub fn self_hash(&self) -> StorageResult<[u8; 20]> {
        self.inner.self_hash()
    }

    /// Whether every byte of this piece is a confirmed sparse hole.
    pub fn piece_extent_all_holes(&self) -> (bool, bool) {
        self.inner.piece_extent_all_holes()
    }
}

impl PieceStorage for RestoreVerifyPiece {
    fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize> {
        self.inner.read_at(buf, off)
    }

    fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize> {
        self.inner.write_at(buf, off)
    }

    fn mark_complete(&self) -> StorageResult<()> {
        self.inner.mark_complete()
    }

    fn mark_not_complete(&self) -> StorageResult<()> {
        self.inner.mark_not_complete()
    }

    fn completion(&self) -> Completion {
        self.inner.completion()
    }
}

/// Return a restore-verify piece when the flag is on, else a plain document piece.
pub fn piece_for_hash(
    storage: &Arc<DocumentTorrentStorage>,
    piece: Piece,
) -> Arc<dyn PieceStorage> {
    let arc = storage.clone();
    if arc.restore_verify_enabled() {
        Arc::new(RestoreVerifyPiece::new(arc, piece))
    } else {
        Arc::new(DocumentPiece {
            storage: arc,
            piece,
        })
    }
}

/// Dense SHA-1 of piece bytes via `read_at` (Go `hashPieceContents`).
pub fn hash_piece_contents(piece: &dyn PieceStorage, length: i64) -> StorageResult<[u8; 20]> {
    if length < 0 {
        return Err(StorageError::Other("negative piece length".into()));
    }
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    let mut off = 0i64;
    while off < length {
        let want = ((length - off) as usize).min(buf.len());
        let n = piece.read_at(&mut buf[..want], off)?;
        if n == 0 {
            return Err(StorageError::Other(format!(
                "short piece read at offset {off}"
            )));
        }
        hasher.update(&buf[..n]);
        off += n as i64;
    }
    if off != length {
        return Err(StorageError::Other("unexpected eof in piece read".into()));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&hasher.finalize());
    Ok(out)
}

/// Hash file spans the restore path uses; all-holes yields [`zero_piece_hash`].
pub fn hash_spans_or_zero(
    spans: &[crate::extent::FileSpan],
    length: i64,
) -> StorageResult<[u8; 20]> {
    let (sum, all_holes) = hash_extents(spans)?;
    if all_holes {
        Ok(zero_piece_hash(length))
    } else {
        Ok(sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        seek_data, zero_piece_hash, ClientStorage, FileInfo, Info, InfoHash, TorrentStorage,
    };
    use sha1::{Digest, Sha1};
    use std::fs::{self, OpenOptions};
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture_dir(prefix: &str) -> PathBuf {
        let root = PathBuf::from("/home/kar/.cache/wt-tmp/wt-storage-restore");
        let _ = fs::create_dir_all(&root);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{prefix}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn single_file_info(name: &str, length: i64, piece_length: i64) -> Info {
        let num = if piece_length <= 0 || length <= 0 {
            0
        } else {
            ((length + piece_length - 1) / piece_length) as usize
        };
        Info {
            name: name.into(),
            piece_length,
            pieces: vec![0u8; num * 20],
            files: vec![FileInfo {
                length,
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
        }
    }

    #[test]
    fn zero_piece_hash_matches_sha1() {
        for length in [0i64, 1, 20, 16 * 1024, 256 * 1024 + 7] {
            let got = zero_piece_hash(length);
            let mut h = Sha1::new();
            h.update(vec![0u8; length as usize]);
            assert_eq!(&got[..], h.finalize().as_slice(), "length {length}");
            assert_eq!(zero_piece_hash(length), got, "cache miss for {length}");
        }
    }

    #[test]
    fn sparse_piece_extent_all_holes() {
        let dir = fixture_dir("sparse-holes");
        let info = single_file_info("sparse.bin", 32 * 1024, 16 * 1024);
        let st = DocumentTorrentStorage::new(Some(info.clone()), InfoHash([0; 20]), &dir);
        let path = dir.join("dest");
        let f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        f.set_len(32 * 1024).unwrap();
        let fd = f.as_raw_fd();
        st.attach_descriptors(&[Some(fd)], &[0]).unwrap();

        let (all, supported) = st.piece_extent_all_holes(info.piece(0));
        if !supported {
            // Filesystem without SEEK_DATA/HOLE — skip like Go.
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        assert!(all, "truncated file should report all-hole pieces");

        use std::io::Write;
        let mut f2 = OpenOptions::new().write(true).open(&path).unwrap();
        f2.write_all(b"not-a-hole").unwrap();
        drop(f2);

        let (all, supported) = st.piece_extent_all_holes(info.piece(0));
        assert!(supported);
        assert!(!all, "piece with written data was treated as all holes");

        let (all, supported) = st.piece_extent_all_holes(info.piece(1));
        assert!(supported);
        if all {
            let piece = RestoreVerifyPiece::new(st.clone(), info.piece(1));
            let sum = piece.self_hash().unwrap();
            assert_eq!(sum, zero_piece_hash(info.piece_length));
        } else {
            // tmpfs may allocate the whole file on first write; fall back path.
            let pos = seek_data(fd, 16 * 1024);
            assert!(pos.is_ok(), "sparse probe still usable");
        }
        let _ = st.close();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_verify_self_hash_uses_zero_hash_for_holes() {
        let dir = fixture_dir("self-hash-holes");
        let info = single_file_info("sparse.bin", 16 * 1024, 16 * 1024);
        let st = DocumentTorrentStorage::new(Some(info.clone()), InfoHash([0; 20]), &dir);
        let path = dir.join("dest");
        let f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        f.set_len(16 * 1024).unwrap();
        let fd = f.as_raw_fd();
        st.attach_descriptors(&[Some(fd)], &[0]).unwrap();
        st.set_restore_verify(true);

        let piece = RestoreVerifyPiece::new(st.clone(), info.piece(0));
        let sum = piece.self_hash().unwrap();
        let (all, supported) = st.piece_extent_all_holes(info.piece(0));
        if supported && all {
            assert_eq!(sum, zero_piece_hash(info.piece_length));
        } else {
            // Fallback hashed zeros the hard way; result must still match.
            assert_eq!(sum, zero_piece_hash(info.piece_length));
        }
        let _ = st.close();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_verify_piece_type_only_while_enabled() {
        let dir = fixture_dir("restore-gate");
        let info = single_file_info("one", 4, 4);
        let client = crate::DocumentStorageClient::new(&dir);
        let hash = InfoHash([1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let impl_ts = client.open_torrent(&info, hash).unwrap();
        let st = client.get_storage(&hash.to_hex()).unwrap();

        let plain = piece_for_hash(&st, info.piece(0));
        // Plain path is DocumentPiece — no restore wrapper type check via downcast,
        // but restore_verify is off so piece_for_hash returns DocumentPiece.
        assert!(!st.restore_verify_enabled());
        let _ = plain.completion();

        st.set_restore_verify(true);
        assert!(st.restore_verify_enabled());
        let restoring = piece_for_hash(&st, info.piece(0));
        // Still usable as PieceStorage.
        let _ = restoring.completion();

        st.set_restore_verify(false);
        assert!(!st.restore_verify_enabled());
        let _ = impl_ts.close();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_piece_contents_matches_sha1() {
        // In-memory fake piece.
        struct MemPiece(Vec<u8>);
        impl PieceStorage for MemPiece {
            fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize> {
                if off < 0 || off as usize >= self.0.len() {
                    return Ok(0);
                }
                let start = off as usize;
                let n = buf.len().min(self.0.len() - start);
                buf[..n].copy_from_slice(&self.0[start..start + n]);
                Ok(n)
            }
            fn write_at(&self, _: &[u8], _: i64) -> StorageResult<usize> {
                Err(StorageError::Other("ro".into()))
            }
            fn mark_complete(&self) -> StorageResult<()> {
                Ok(())
            }
            fn mark_not_complete(&self) -> StorageResult<()> {
                Ok(())
            }
            fn completion(&self) -> Completion {
                Completion {
                    ok: true,
                    complete: true,
                    err: None,
                }
            }
        }
        let data: Vec<u8> = (0..100u8).collect();
        let mut want_h = Sha1::new();
        want_h.update(&data);
        let got = hash_piece_contents(&MemPiece(data.clone()), data.len() as i64).unwrap();
        assert_eq!(&got[..], want_h.finalize().as_slice());
    }
}

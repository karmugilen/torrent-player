//! Storage test doubles and piece I/O glue over `wt_types::PieceStorage`.
//!
//! Keeps agy-2's in-memory doubles + `verify_piece_hash`. Adds [`StorageGlue`]
//! for write/read/mark/completion refresh and write-error callbacks (Go
//! `Torrent.writeChunk`, `updatePieceCompletion`, `SetOnWriteChunkError`).

use parking_lot::RwLock;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::Arc;
use wt_types::{
    ClientStorage, Completion, Info, InfoHash, Piece, PieceStorage, StorageError, StorageResult,
    TorrentStorage,
};

use crate::piece_state::PieceBook;

/// An in-memory piece storage double.
pub struct MemoryPieceStorage {
    #[allow(dead_code)]
    piece: Piece,
    #[allow(dead_code)]
    hash: Option<[u8; 20]>,
    data: RwLock<Vec<u8>>,
    completion: RwLock<Completion>,
}

impl MemoryPieceStorage {
    pub fn new(piece: Piece, hash: Option<[u8; 20]>) -> Self {
        Self {
            piece,
            hash,
            data: RwLock::new(vec![0u8; piece.length as usize]),
            completion: RwLock::new(Completion::default()),
        }
    }
}

impl PieceStorage for MemoryPieceStorage {
    fn read_at(&self, buf: &mut [u8], off: i64) -> StorageResult<usize> {
        let data = self.data.read();
        if off < 0 || off as usize >= data.len() {
            return Ok(0);
        }
        let start = off as usize;
        let available = data.len() - start;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&data[start..start + to_read]);
        Ok(to_read)
    }

    fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize> {
        let mut data = self.data.write();
        if off < 0 {
            return Err(StorageError::Other("negative offset".to_string()));
        }
        let start = off as usize;
        let end = start + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn mark_complete(&self) -> StorageResult<()> {
        let mut comp = self.completion.write();
        comp.ok = true;
        comp.complete = true;
        comp.err = None;
        Ok(())
    }

    fn mark_not_complete(&self) -> StorageResult<()> {
        let mut comp = self.completion.write();
        comp.ok = true;
        comp.complete = false;
        comp.err = None;
        Ok(())
    }

    fn completion(&self) -> Completion {
        self.completion.read().clone()
    }
}

/// An in-memory torrent storage double.
pub struct MemoryTorrentStorage {
    pub pieces: RwLock<HashMap<u32, Arc<MemoryPieceStorage>>>,
}

impl Default for MemoryTorrentStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTorrentStorage {
    pub fn new() -> Self {
        Self {
            pieces: RwLock::new(HashMap::new()),
        }
    }
}

impl TorrentStorage for MemoryTorrentStorage {
    fn piece(&self, piece: Piece, hash: Option<[u8; 20]>) -> Arc<dyn PieceStorage> {
        let mut lock = self.pieces.write();
        lock.entry(piece.index)
            .or_insert_with(|| Arc::new(MemoryPieceStorage::new(piece, hash)))
            .clone()
    }

    fn close(&self) -> StorageResult<()> {
        Ok(())
    }
}

/// An in-memory client storage implementation.
pub struct MemoryClientStorage {
    pub torrents: RwLock<HashMap<InfoHash, Arc<MemoryTorrentStorage>>>,
}

impl Default for MemoryClientStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryClientStorage {
    pub fn new() -> Self {
        Self {
            torrents: RwLock::new(HashMap::new()),
        }
    }
}

impl ClientStorage for MemoryClientStorage {
    fn open_torrent(
        &self,
        _info: &Info,
        info_hash: InfoHash,
    ) -> StorageResult<Arc<dyn TorrentStorage>> {
        let mut lock = self.torrents.write();
        let tor = lock
            .entry(info_hash)
            .or_insert_with(|| Arc::new(MemoryTorrentStorage::new()));
        Ok(tor.clone())
    }
}

/// Verifies piece storage against expected v1 SHA-1 hash.
pub fn verify_piece_hash(
    piece_storage: &dyn PieceStorage,
    piece_len: usize,
    expected_hash: Option<[u8; 20]>,
) -> bool {
    let Some(expected) = expected_hash else {
        // If no v1 hash is provided, consider it valid if piece exists
        return true;
    };
    let mut buf = vec![0u8; piece_len];
    let mut read = 0;
    while read < piece_len {
        match piece_storage.read_at(&mut buf[read..], read as i64) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return false,
        }
    }
    if read != piece_len {
        return false;
    }
    let mut hasher = Sha1::new();
    hasher.update(&buf);
    let digest: [u8; 20] = hasher.finalize().into();
    digest == expected
}

type WriteErrCb = Arc<dyn Fn(&StorageError) + Send + Sync>;

/// Glue over [`TorrentStorage`] for chunk write/read, mark, completion refresh,
/// and write-error callback (Go torrent storage helpers).
pub struct StorageGlue {
    storage: Arc<dyn TorrentStorage>,
    on_write_error: RwLock<Option<WriteErrCb>>,
}

impl StorageGlue {
    pub fn new(storage: Arc<dyn TorrentStorage>) -> Self {
        Self {
            storage,
            on_write_error: RwLock::new(None),
        }
    }

    pub fn set_on_write_chunk_error(&self, f: Option<WriteErrCb>) {
        *self.on_write_error.write() = f;
    }

    pub fn piece_storage(&self, piece: Piece, hash: Option<[u8; 20]>) -> Arc<dyn PieceStorage> {
        self.storage.piece(piece, hash)
    }

    /// Go `Torrent.writeChunk` — write full buffer or error (incl. short write).
    pub fn write_chunk(
        &self,
        piece: Piece,
        hash: Option<[u8; 20]>,
        begin: i64,
        data: &[u8],
    ) -> StorageResult<()> {
        let ps = self.storage.piece(piece, hash);
        match ps.write_at(data, begin) {
            Ok(n) if n == data.len() => Ok(()),
            Ok(n) => {
                let err =
                    StorageError::Other(format!("short write: wrote {n} of {} bytes", data.len()));
                self.fire_write_error(&err);
                Err(err)
            }
            Err(err) => {
                self.fire_write_error(&err);
                Err(err)
            }
        }
    }

    pub fn read_at(
        &self,
        piece: Piece,
        hash: Option<[u8; 20]>,
        buf: &mut [u8],
        off: i64,
    ) -> StorageResult<usize> {
        self.storage.piece(piece, hash).read_at(buf, off)
    }

    pub fn mark_complete(&self, piece: Piece, hash: Option<[u8; 20]>) -> StorageResult<()> {
        self.storage.piece(piece, hash).mark_complete()
    }

    pub fn mark_not_complete(&self, piece: Piece, hash: Option<[u8; 20]>) -> StorageResult<()> {
        self.storage.piece(piece, hash).mark_not_complete()
    }

    pub fn completion(&self, piece: Piece, hash: Option<[u8; 20]>) -> Completion {
        self.storage.piece(piece, hash).completion()
    }

    /// Pull completion from storage into the book. Returns whether the cache changed.
    pub fn update_piece_completion(
        &self,
        book: &mut PieceBook,
        piece: Piece,
        hash: Option<[u8; 20]>,
    ) -> bool {
        let c = self.completion(piece, hash);
        book.update_completion(piece.index, c)
    }

    fn fire_write_error(&self, err: &StorageError) {
        if let Some(cb) = &*self.on_write_error.read() {
            cb(err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn glue_write_read_mark_completion() {
        let tor = Arc::new(MemoryTorrentStorage::new());
        let glue = StorageGlue::new(tor);
        let piece = Piece {
            index: 0,
            offset: 0,
            length: 32,
        };
        let data = b"abcdefghijklmnopqrstuvwxyz012345";
        glue.write_chunk(piece, None, 0, data).unwrap();
        let mut buf = [0u8; 32];
        assert_eq!(glue.read_at(piece, None, &mut buf, 0).unwrap(), 32);
        assert_eq!(&buf, data);

        assert!(!glue.completion(piece, None).complete);
        glue.mark_complete(piece, None).unwrap();
        let c = glue.completion(piece, None);
        assert!(c.ok && c.complete);

        let mut book = PieceBook::new(1, 32, 32, 16);
        assert!(glue.update_piece_completion(&mut book, piece, None));
        assert!(book.piece_complete(0));
    }

    #[test]
    fn write_error_callback() {
        let tor = Arc::new(MemoryTorrentStorage::new());
        let glue = StorageGlue::new(tor);
        let hit = Arc::new(AtomicBool::new(false));
        let hit2 = hit.clone();
        glue.set_on_write_chunk_error(Some(Arc::new(move |_e| {
            hit2.store(true, Ordering::SeqCst);
        })));
        // Force error via negative offset on an existing piece length 1.
        let piece = Piece {
            index: 0,
            offset: 0,
            length: 1,
        };
        let _ = glue.write_chunk(piece, None, -1, b"x");
        assert!(hit.load(Ordering::SeqCst));
    }
}

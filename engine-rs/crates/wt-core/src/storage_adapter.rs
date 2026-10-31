//! Storage test doubles and piece I/O glue over `wt_types::PieceStorage`.
use parking_lot::RwLock;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::Arc;
use wt_types::{
    ClientStorage, Completion, Info, InfoHash, Piece, PieceStorage, StorageError, StorageResult,
    TorrentStorage,
};


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

    fn write_at_in_memory(&self, buf: &[u8], off: i64) -> Option<StorageResult<usize>> {
        Some(self.write_at(buf, off))
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


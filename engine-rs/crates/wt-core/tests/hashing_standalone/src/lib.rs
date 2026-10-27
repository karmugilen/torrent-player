//! Standalone compile of wt-core hashing + webseed_peer while agy-2 mid-edits the crate.

use std::sync::Arc;

pub use wt_types::*;

#[derive(Clone, Default)]
pub struct CancelToken(pub Arc<std::sync::atomic::AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub mod storage_adapter {
    use parking_lot::RwLock;
    use wt_types::{Completion, Piece, PieceStorage, StorageResult};

    pub struct MemoryPieceStorage {
        data: RwLock<Vec<u8>>,
        completion: RwLock<Completion>,
    }
    impl MemoryPieceStorage {
        pub fn new(piece: Piece, _hash: Option<[u8; 20]>) -> Self {
            Self {
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
            let to_read = buf.len().min(data.len() - start);
            buf[..to_read].copy_from_slice(&data[start..start + to_read]);
            Ok(to_read)
        }
        fn write_at(&self, buf: &[u8], off: i64) -> StorageResult<usize> {
            let mut data = self.data.write();
            if off < 0 {
                return Err(wt_types::StorageError::Other("neg".into()));
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
            let mut c = self.completion.write();
            c.ok = true;
            c.complete = true;
            Ok(())
        }
        fn mark_not_complete(&self) -> StorageResult<()> {
            let mut c = self.completion.write();
            c.ok = true;
            c.complete = false;
            Ok(())
        }
        fn completion(&self) -> Completion {
            self.completion.read().clone()
        }
    }
}

#[path = "../../../src/hashing/mod.rs"]
pub mod hashing;

#[path = "../../../src/webseed_peer/mod.rs"]
pub mod webseed_peer;

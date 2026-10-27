use crate::storage::{DocumentPiece, DocumentTorrentStorage};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wt_types::{Info, StorageError, StorageResult};

pub struct VerificationProgress {
    pub checking: AtomicBool,
    pub checked_pieces: AtomicUsize,
    pub check_total: usize,
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
}

impl VerificationProgress {
    pub fn new(total: usize) -> Self {
        Self {
            checking: AtomicBool::new(false),
            checked_pieces: AtomicUsize::new(0),
            check_total: total,
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }
}

pub fn check_saved_data(
    storage: Arc<DocumentTorrentStorage>,
    info: &Info,
    selected_files: &[usize],
    progress: Arc<VerificationProgress>,
) -> StorageResult<()> {
    let mut pieces = Vec::new();
    let num_pieces = info.num_pieces() as usize;

    for &f_idx in selected_files {
        if f_idx >= info.files.len() {
            continue;
        }
        let f = &info.files[f_idx];
        if f.length <= 0 || info.piece_length <= 0 {
            continue;
        }
        let begin = (f.torrent_offset / info.piece_length) as usize;
        let last = (((f.torrent_offset + f.length - 1) / info.piece_length) as usize)
            .min(num_pieces.saturating_sub(1));
        for p in begin..=last {
            if !pieces.contains(&p) {
                pieces.push(p);
            }
        }
    }
    pieces.sort_unstable();

    progress.checking.store(true, Ordering::SeqCst);
    storage.set_restore_verify(true);

    let mut pending = Vec::new();
    let mut trusted = 0;
    for &p in &pieces {
        if storage.piece_complete(p) {
            trusted += 1;
        } else {
            pending.push(p);
        }
    }

    progress.checked_pieces.store(trusted, Ordering::SeqCst);

    if pending.is_empty() {
        progress.checking.store(false, Ordering::SeqCst);
        storage.mark_verified();
        storage.unfreeze_trusted();
        storage.set_restore_verify(false);
        return Ok(());
    }

    for &p_idx in &pending {
        while progress.paused.load(Ordering::SeqCst) {
            if progress.cancelled.load(Ordering::SeqCst) {
                storage.set_restore_verify(false);
                storage.unfreeze_trusted();
                progress.checking.store(false, Ordering::SeqCst);
                return Err(StorageError::Closed);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if progress.cancelled.load(Ordering::SeqCst) {
            storage.set_restore_verify(false);
            storage.unfreeze_trusted();
            progress.checking.store(false, Ordering::SeqCst);
            return Err(StorageError::Closed);
        }

        let piece = info.piece(p_idx as u32);
        let doc_piece = DocumentPiece {
            storage: storage.clone(),
            piece,
        };

        if let Ok(hash) = doc_piece.self_hash() {
            if let Some(expected) = info.piece_hash_v1(p_idx as u32) {
                if hash == expected {
                    let _ = doc_piece.mark_complete();
                } else {
                    let _ = doc_piece.mark_not_complete();
                }
            }
        } else {
            let _ = doc_piece.mark_not_complete();
        }

        progress.checked_pieces.fetch_add(1, Ordering::SeqCst);
    }

    progress.checking.store(false, Ordering::SeqCst);
    storage.mark_verified();
    storage.unfreeze_trusted();
    storage.set_restore_verify(false);

    Ok(())
}

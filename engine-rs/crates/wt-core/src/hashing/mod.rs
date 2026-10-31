//! # Piece hashing / verification
mod pool;
mod sha1_backend;
mod smartban;
mod verify;

pub use pool::{
    HashJob, HashQueueError, HashResult, PieceHasherPool, SmartBanPieceRange,
    DEFAULT_HASHERS_PER_TORRENT,
};
pub use sha1_backend::{
    hash_v1_default, hash_v1_with_pad, selected_backend, throughput_mib_s, PieceSha1, RingSha1,
    Sha1BackendKind, SoftSha1,
};
pub use smartban::{BannableAddr, SmartBanCache};
pub use verify::{hash_piece_sync, sha1_throughput_mib_s, ExpectedHash, HashWorkError};

//! Engine storage: download, Direct Play RAM cache, checkpoints, sparse extents,
//! heat-grid piece aggregation, and verification.
//!
//! Exposes the same knobs the engine calls: `SetCacheLimit`, `SetMemoryLimit`,
//! `SetRestoreVerify`, file spans/extents, piece aggregation buckets used by the pieces
//! heat grid API (`FileSpan`, `PieceRun`, `PieceRange`, `PieceBucket`, `ByteRange`).

pub mod cache_lru;
pub mod cache_store;
pub mod checkpoint;
pub mod direct_storage;
pub mod extent;
pub mod pieces;
pub mod ranges;
pub mod restore_hash;
pub mod sparse;
pub mod storage;
pub mod verification;
pub mod zero_hash;

// Re-export key types and traits
pub use cache_lru::PieceLRU;
pub use cache_store::{
    clear_cache_categories, delete_resume_window, dht_contacts_path, dir_bytes, file_size,
    format_time, is_valid_peer_addr, load_resume_window, parse_time, peer_hints_path,
    resume_checksum, resume_window_dir, save_resume_window, CacheCategoryView, CacheStatusResponse,
    DhtContact, DhtContactFile, DhtContactStore, PeerHint, PeerHintFile, PeerHintStore,
    ResumeWindowMeta, DHT_CONTACT_MAX_LOAD_BYTES, DHT_CONTACT_MAX_PER_FAMILY, DHT_CONTACT_TTL,
    PEER_HINT_MAX_LOAD_BYTES, PEER_HINT_MAX_PER_TORRENT, PEER_HINT_MAX_TORRENTS, PEER_HINT_TTL,
    RESUME_WINDOW_MAX_BYTES, RESUME_WINDOW_TOTAL_MAX, RESUME_WINDOW_TTL, RESUME_WINDOW_VERSION,
};
pub use checkpoint::{
    checkpoint_path, delete_piece_checkpoint, file_identity_equal, file_stable, identities_match,
    identity_from_file, identity_from_metadata, read_piece_checkpoint, write_piece_checkpoint,
    FileIdentity, PieceCheckpoint, CHECKPOINT_MAGIC, CHECKPOINT_VERSION, MAX_CHECKPOINT_FILES,
    MAX_CHECKPOINT_PATH, MAX_CHECKPOINT_PIECES,
};
pub use extent::{hash_extents, pread_all, pread_extents, FileSpan};
pub use pieces::{
    aggregate, aggregate_piece_buckets, compress_piece_runs, PieceBucket, PieceRange, PieceRun,
    PieceStateRun,
};
pub use ranges::{merge_received_range, ranges_cover, ByteRange};
pub use sparse::{file_occupied_bytes, punch_hole, seek_data, seek_hole};
pub use storage::{
    delete_engine_owned_storage, engine_owned_part_path, is_video_file_name, startup_file_pieces,
    DocumentPiece, DocumentStorageClient, DocumentTorrentStorage, FileDescriptor,
    DEFAULT_MAX_CACHE_BYTES, DEFAULT_STREAM_CACHE_MAX_BYTES, DIRECT_PLAY_MIN_PIECES,
    DIRECT_PRELOAD_BUDGET, DIRECT_STARTUP_BUDGET, DIRECT_STARTUP_HEAD, DIRECT_STARTUP_TAIL,
    DIRECT_STARTUP_TOTAL_BUDGET, MAX_RAM_CACHE_BYTES, MEMORY_LIMIT_OVERHEAD, MIN_RAM_CACHE_BYTES,
    PREVIEW_BUDGET_BYTES, STREAM_CACHE_KEEP_BYTES,
};
pub use verification::{check_saved_data, VerificationProgress};
pub use zero_hash::zero_piece_hash;

// Direct Play facade + restore SelfHash (delegated modules)
pub use direct_storage::{
    apply_auto_ram_cache, apply_cache_limit, auto_ram_cache_bytes, auto_ram_cache_mib,
    background_workers, clamp_ram_cache_bytes, direct_play_min_cache_bytes, open_direct_play,
    piece_fits_direct_play, planned_startup_pieces, prune_startup_caches, register_startup_cache,
    unregister_startup_cache, AUTO_RAM_MAX_MIB, AUTO_RAM_MIN_MIB, AUTO_RAM_STEPS_MIB,
};
pub use restore_hash::{
    hash_piece_contents, hash_spans_or_zero, piece_for_hash, RestoreVerifyPiece,
};

// Re-export storage traits from wt-types
pub use wt_types::{
    ClientStorage, Completion, FileInfo, Info, InfoHash, Piece, PiecePriority, PieceStorage,
    RequestIndex, StorageError, StorageResult, TorrentStorage, BLOCK_SIZE,
};

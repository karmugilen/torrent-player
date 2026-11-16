//! # Piece hashing / verification
mod hash_exchange;
mod leaf_verify;
mod piece_layers;
mod pool;
mod sha1_backend;
mod smartban;
mod verify;

pub use hash_exchange::{
    apply_inbound_hashes, base_layer_for_piece_length, compact_layer_from_expected,
    file_by_pieces_root, hash_requests_for_file, hashes_match_pieces_root, missing_layer_files,
    piece_pad_hash, proof_uncles, serve_hashes, serve_hashes_at_layer, HashRequestKey,
    MissingLayerFile, HASH_REQUEST_CHUNK, MAX_HASH_REQUEST_LENGTH, MAX_OUTSTANDING_HASH_REQUESTS,
};
pub use leaf_verify::{
    check_block, leaf_hash, leaf_layer_matches, leaf_layer_of, leaf_request_for_piece,
    piece_for_leaf_span, LeafCheck,
};
pub use piece_layers::{build_piece_layer_index, expected_v2_for_piece};
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

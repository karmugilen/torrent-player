//! Metainfo, bencode, magnets, and BEP 52 merkle trees.
#![deny(unsafe_code)]

pub mod bencode;
pub mod error;
pub mod magnet;
pub mod merkle;
pub mod metainfo;

pub use error::{Error, Result};
pub use magnet::{magnet_uri, parse_magnet};
pub use metainfo::{
    choose_piece_length, encode_info, encode_metainfo, info_hash_v1, info_hash_v2, parse_info,
    parse_metainfo, parse_torrent, validate_metainfo_piece_layers,
};

// Re-export the shared data model so downstream crates can depend on wt-meta alone
// for metainfo work.
pub use wt_types::{FileInfo, Info, InfoHash, InfoHashV2, Magnet, MetaInfo};

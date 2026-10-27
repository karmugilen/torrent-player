//! Shared fixtures for wt-engine integration tests.
//!
//! Mirrors Go `testEngine` / playback fixtures: temp download dir, control
//! dispatch helpers, and deterministic .torrent builders via `wt-meta`.

#![allow(dead_code, unused_imports)]

pub mod engine;
pub mod torrent;

pub use engine::{json_get, json_post, TestEngine};
pub use torrent::{
    build_multi_file_torrent, build_single_file_torrent, info_hash_hex, piece_payload,
};

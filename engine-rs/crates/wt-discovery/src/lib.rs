//! Peer discovery for the Webtor Rust engine.
//!
//! Ports `engine-go/patches/torrent/{tracker,webtorrent,webseed}`,
//! `client-tracker-announcer.go`, DHT usage (`dht.go` / anacrolix/dht v2),
//! and `bep40.go`. See `PORT_NOTES.md` and `engine-rs/docs/PORT_MAP.md`.

pub mod bep40;
pub mod dht;
pub mod status;
pub mod tracker;
pub mod webseed;
pub mod webtorrent;

pub use bep40::priority as bep40_priority;
pub use status::{StatusCallback, TrackerStatusEvent};

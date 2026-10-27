//! Peer discovery for the Webtor Rust engine.
//!
//! Ports `engine-go/patches/torrent/{tracker,webtorrent,webseed}`,
//! `client-tracker-announcer.go`, DHT usage (`dht.go` / anacrolix/dht v2),
//! and `bep40.go`. See `PORT_NOTES.md` and `engine-rs/docs/PORT_MAP.md`.

use std::sync::Once;

/// Install the rustls `ring` CryptoProvider once (safe to call repeatedly).
pub fn ensure_rustls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub mod bep40;
pub mod datagram;
pub mod dht;
pub mod status;
pub mod tracker;
pub mod webseed;
pub mod webtorrent;

pub use bep40::priority as bep40_priority;
pub use datagram::{DatagramIo, StandaloneDatagram};
pub use status::{StatusCallback, TrackerStatusEvent};

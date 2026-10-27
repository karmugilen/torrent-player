//! # Choking / unchoking (agy-2 integration guide)
//!
//! Pure choke decisions + BEP 6 allowed-fast set generation. Wire I/O
//! (sending Choke/Unchoke/AllowedFast messages) stays in peer_conn.
//!
//! ## What agy-2 should call
//!
//! ```ignore
//! use wt_core::choking::{
//!     ChokeConfig, ChokeState, PeerChokeInput, ChokeDecision,
//!     compute_choke_decisions, upload_allowed, generate_allowed_fast_set,
//!     DEFAULT_UPLOAD_SLOTS, OPTIMISTIC_UNCHOKE_INTERVAL,
//! };
//! use std::net::Ipv4Addr;
//! use std::time::Instant;
//!
//! // Per torrent/client: keep ChokeState, tick on a 10s timer.
//! let mut state = ChokeState::new(Instant::now());
//! let config = ChokeConfig {
//!     upload_slots: DEFAULT_UPLOAD_SLOTS,
//!     seeding: torrent.is_seeding(),
//!     data_upload_disallowed: torrent.data_upload_disallowed(),
//!     ..ChokeConfig::default()
//! };
//! let decisions = compute_choke_decisions(&peers, &config, &mut state, Instant::now());
//! for (peer_id, d) in decisions {
//!     match d {
//!         ChokeDecision::Unchoke => { /* send Unchoke if currently choking */ }
//!         ChokeDecision::Choke => { /* send Choke if currently unchoking */ }
//!     }
//! }
//!
//! // Optional: Go-compatible per-peer tit-for-tat gate (anacrolix uploadAllowed).
//! if upload_allowed(&peer, &config, peer_has_wanted_pieces) { /* may unchoke */ }
//!
//! // On fast-extension handshake: send AllowedFast for each index.
//! let set = generate_allowed_fast_set(peer_ipv4, &infohash, num_pieces, 10);
//! ```
//!
//! Seed policy: `engine-go/seeding.go` `applySeedPolicy` decides
//! AllowDataUpload / DisallowDataUpload; pass the result as
//! `config.data_upload_disallowed`. This module only does per-peer choke picks.

mod allowed_fast;
mod seed_policy;
mod unchoke;

pub use allowed_fast::generate_allowed_fast_set;
pub use seed_policy::upload_allowed;
pub use unchoke::{
    compute_choke_decisions, ChokeConfig, ChokeDecision, ChokeState, PeerChokeInput,
    DEFAULT_UPLOAD_SLOTS, OPTIMISTIC_UNCHOKE_INTERVAL, REGULAR_UNCHOKE_INTERVAL,
    TIT_FOR_TAT_HEADROOM,
};

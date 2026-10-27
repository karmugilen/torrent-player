//! Standalone compile of `wt-core::piece_state` + `storage_adapter` while the
//! parent crate is mid-edit (agy-2 peer_conn / torrent).

pub use wt_types::*;

#[path = "../../../src/piece_state/mod.rs"]
pub mod piece_state;

#[path = "../../../src/storage_adapter.rs"]
pub mod storage_adapter;

/// Thin `wt_core` facade so integration tests can `use wt_core::...`.
pub mod wt_core_facade {
    pub use crate::piece_state;
    pub use crate::storage_adapter;
    pub use wt_types::*;
}

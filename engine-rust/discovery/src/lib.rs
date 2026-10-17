//! Fast torrents.csv dump index and catalog fetch for Torrent Atlas discovery.
//!
//! Linked into `engine-go/cmd/discovery` via cgo (`rustdump` build tag).

mod dump;
mod fetch;
mod ffi;

pub use dump::{DumpIndex, DumpRow};
pub use fetch::{ensure_dump, fetch_catalog_page, CatalogPage, CatalogTorrent};

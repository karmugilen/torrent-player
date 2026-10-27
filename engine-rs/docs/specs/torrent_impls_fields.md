# Fields for agy-2 to add on TorrentInner (pub(crate))

Needed so `torrent_impls.rs` can impl `PeerTorrent` + `ReaderHost`.
Also: please ask grok-8 to **remove** the existing `impl ReaderHost for TorrentInner` from `reader.rs` (and the stopgap helper) — ownership of that impl moves to `torrent_impls.rs`.

```rust
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::Instant;
use parking_lot::RwLock;
use std::sync::Arc;

use crate::piece_state::{Bitset, PieceBook};
use crate::storage_adapter::StorageGlue;
use crate::hashing::{HashResult, PieceHasherPool};
use crate::metadata::MetadataExchange;
use crate::pex::PexState;
use crate::peer_conn::PeerKey;
use crate::choking::ChokeConfig;
use crate::strategy::PieceRequestOrder;
use crate::reader::PieceRange;
use wt_types::RequestIndex;

// --- piece_state / storage ---
pub(crate) book: RwLock<Option<PieceBook>>,
pub(crate) glue: RwLock<Option<StorageGlue>>,

// --- readers (ReaderHost) ---
pub(crate) reader_ranges: RwLock<HashMap<u64, PieceRange>>,

// --- hashing ---
pub(crate) hasher: RwLock<Option<Arc<PieceHasherPool>>>,
pub(crate) hash_seq: AtomicU64,
// optional: pub(crate) hash_rx: Mutex<Option<mpsc::Receiver<HashResult>>>,

// --- peer identity + assignments (PeerTorrent) ---
pub(crate) peer_key_by_addr: RwLock<HashMap<SocketAddr, PeerKey>>,
pub(crate) addr_by_peer_key: RwLock<HashMap<PeerKey, SocketAddr>>,
pub(crate) next_peer_key: AtomicU64,
pub(crate) assignments: RwLock<HashMap<RequestIndex, (PeerKey, Instant)>>,
pub(crate) hedge_peers: RwLock<HashMap<RequestIndex, PeerKey>>,
pub(crate) peer_pieces: RwLock<HashMap<PeerKey, Bitset>>, // or HashSet<PieceIndex>

// --- strategy / choking ---
pub(crate) piece_request_order: RwLock<PieceRequestOrder>,
pub(crate) choke_config: RwLock<ChokeConfig>,

// --- metadata / pex / discovery hooks ---
pub(crate) metadata: RwLock<MetadataExchange>,
pub(crate) pex: RwLock<PexState>,
pub(crate) pex_last_recv: RwLock<HashMap<SocketAddr, Instant>>,
pub(crate) dht_nodes: RwLock<Vec<SocketAddr>>,
pub(crate) peer_listen_ports: RwLock<HashMap<SocketAddr, u16>>,
```

Init notes for `TorrentInner::new` / `set_info`:
- `book`/`glue` created in `set_info` once Info + TorrentStorage exist (`PieceBook::new(...)`, `StorageGlue::new(storage)`).
- `metadata`: `MetadataExchange::new(Some(info_hash), info_hash_v2)`.
- `hasher`: optional until client wires pool; `queue_piece_hash` no-ops if None.
- Keep existing `dirty_chunks`/`pieces`/`hedges`/`request_state` until agy-2 migrates; `torrent_impls` prefers `book`/`assignments`/`hedge_peers` when present.

Existing fields already used (no change needed):
`info`, `storage`, `info_hash`, `conns`, `request_focus`, `data_*_disallowed`, `seeding`, `closed`, `change_version`, `change_cond`, `change_mutex`, `rescues`, `hedges_sent`, `on_write_chunk_error`, `arrival_model`, `max_established_conns`.

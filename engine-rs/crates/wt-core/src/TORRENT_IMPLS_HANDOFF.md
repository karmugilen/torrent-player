# HANDOFF: torrent_impls (PeerTorrent + ReaderHost)

## Done
- `src/torrent_impls.rs`: full `impl PeerTorrent for TorrentInner` + `impl ReaderHost for TorrentInner`.
- `pub mod torrent_impls;` in `lib.rs`.
- Removed prior `impl ReaderHost` from `reader.rs` (coordinator-approved move).
- Helpers: `init_book_and_glue`, `note_peer_piece`, `peer_key_from_addr`.
- Tests: `tests/torrent_impls_peer_reader.rs` (need fields to compile).
- Notes: `TORRENT_IMPLS_PORT_NOTES.md`.

## Fields assumed on TorrentInner (pub(crate) or pub) — agy-2 must add

Already present: `reader_ranges: RwLock<HashMap<u64, PieceRange>>`.

Still missing:
```
book: RwLock<Option<PieceBook>>,
glue: RwLock<Option<StorageGlue>>,
hasher: RwLock<Option<PieceHasherPool>>,
hash_seq: AtomicU64,
peer_key_by_addr: RwLock<HashMap<SocketAddr, PeerKey>>,
addr_by_peer_key: RwLock<HashMap<PeerKey, SocketAddr>>,
next_peer_key: AtomicU64,
assignments: RwLock<HashMap<RequestIndex, (PeerKey, Instant)>>,
hedge_peers: RwLock<HashMap<RequestIndex, PeerKey>>,
peer_pieces: RwLock<HashMap<PeerKey, piece_state::Bitset>>,
piece_request_order: RwLock<PieceRequestOrder>,
choke_config: RwLock<ChokeConfig>,
metadata: RwLock<MetadataExchange>,
pex: RwLock<PexState>,
pex_last_recv: RwLock<HashMap<SocketAddr, Instant>>,
dht_nodes: RwLock<Vec<SocketAddr>>,
peer_listen_ports: RwLock<HashMap<SocketAddr, u16>>,
```

Init: `new` defaults; `set_info` should call `torrent_impls::init_book_and_glue` after opening storage. Construct `metadata: MetadataExchange::new(Some(info_hash), info_hash_v2)`.

## Blocker
1. Missing fields above → `torrent_impls.rs` does not compile.
2. **Conflict:** `torrent.rs` now also has `impl ReaderHost for TorrentInner` (~line 799). That must be deleted; ownership is `torrent_impls.rs` only.

## Method coverage (once fields land)
Fully implemented in torrent_impls: all PeerTorrent + ReaderHost methods.
Thin leftovers for callers: apply verified metadata via set_info; feed dht_nodes to sources; drain hasher results → mark_complete/pend_all; call note_peer_piece from Have/Bitfield.

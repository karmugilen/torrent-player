# HANDOFF grok-9 (PeerTorrent + ReaderHost in torrent_impls)

Worker: grok-9 · task_3d7f24f1eedd · dispatch ctx_415e10a96dc0 · **PAUSED**

## Done
- `src/torrent_impls.rs`: compile-safe `impl PeerTorrent for TorrentInner` against **existing** `TorrentInner` fields (`dirty_chunks`, `pieces`, `storage`, `request_state`, `hedges`, `conns`, flags, change_cond).
- `pub mod torrent_impls;` in `lib.rs` (exact one-line edit).
- Removed prior `impl ReaderHost` from `reader.rs` (pointer comment left; coordinator-approved ownership move).
- Helpers (placeholders until fields land): `init_book_and_glue`, `note_peer_piece`, `peer_key_from_addr`.
- `TorrentInner::as_reader_host` → `Arc<dyn ReaderHost>` (uses agy-2's current impl in `torrent.rs`).
- In-module unit test for `peer_key_from_addr`.
- Notes: `src/TORRENT_IMPLS_PORT_NOTES.md`, `src/TORRENT_IMPLS_HANDOFF.md` (field list; somewhat ahead of current compile-safe code).

## Half-done / deferred (blocked on agy-2)
- Full book/glue/hasher/metadata/PEX-backed PeerTorrent (draft existed; slimmed so crate compiles).
- `impl ReaderHost for TorrentInner` in `torrent_impls.rs` — **not** present here because `torrent.rs` already has a duplicate ~L799 (only `reader_ranges` of the planned fields exists). Restoring ours would be a duplicate-impl error.
- Integration test `tests/torrent_impls_peer_reader.rs` expects full fields (`t.book`, etc.) — will not compile until fields land; treat as resume work.

## Fields still needed on `TorrentInner` (agy-2, `pub(crate)`)
Already present: `reader_ranges: RwLock<HashMap<u64, PieceRange>>`.

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

Init: defaults in `new`; after opening storage in `set_info`, call `torrent_impls::init_book_and_glue`. Construct `metadata: MetadataExchange::new(Some(info_hash), info_hash_v2)`.

**Also required on resume:** delete `impl ReaderHost for TorrentInner` from `torrent.rs` (~L799) so ownership can return to `torrent_impls.rs`.

## PeerTorrent method coverage (current compile-safe file)
| Group | Status |
|---|---|
| Info / lengths / flags | full |
| Chunk dirty via `dirty_chunks` | full |
| write/read via `storage` | full |
| Assignments via `request_state` | full (PeerKey ↔ addr) |
| Hedges via `hedges` | full |
| desired_request_context | full (ephemeral order) |
| peer_has_piece / webtor snap | full (via `conns`) |
| queue_piece_hash | TODO — no hasher pool; sets `checking` on PieceEntry |
| metadata / PEX / DHT / listen ports | TODO stubs |
| choke_config | default + live disallow/seed flags |
| bump_change / on_chunk_received | full |
| ReaderHost | deferred — lives in `torrent.rs` until fields + delete |

## Next steps (on resume)
1. Wait for agy-2 fields + delete of `torrent.rs` ReaderHost.
2. Restore book/glue/hasher/metadata PeerTorrent + ReaderHost in `torrent_impls.rs`.
3. Unblock / fix `tests/torrent_impls_peer_reader.rs`; clippy + fmt.
4. Update PORT_NOTES; `worker_done` with honest coverage.

## Verify (grok-9 target only)
```
cd engine-rs
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-9 CARGO_BUILD_JOBS=3 \
  cargo check -p wt-core
```
Do **not** use other workers' target dirs. Do **not** `worker_done` until acceptance is met.

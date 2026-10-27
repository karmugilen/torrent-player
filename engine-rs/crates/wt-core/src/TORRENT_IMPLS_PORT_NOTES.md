# torrent_impls port notes

Owned: `src/torrent_impls.rs`, `tests/torrent_impls_*.rs`, exact `mod torrent_impls;` in `lib.rs`.
Moved `impl ReaderHost for TorrentInner` out of `reader.rs` (coordinator-approved).

## Ported
- `impl PeerTorrent for TorrentInner` — chunk bookkeeping via `PieceBook`, I/O via `StorageGlue`, hash queue via `PieceHasherPool`, assignments/hedges, desired-request snapshot + `PieceRequestOrder` refresh, Webtor snaps from `PeerConn`, metadata (`MetadataExchange`), PEX (`PexState` → `known_peers`), DHT/listen-port lists, choke config, change bump, `on_chunk_received`.
- `impl ReaderHost for TorrentInner` — responsive/non-responsive `have` over book dirty bits, `wait_change` / version, `reader_ranges` → Now/Readahead on book, `read_at` via glue/storage, `on_read_waiting` → focus + nudge.
- Helpers: `init_book_and_glue`, `note_peer_piece`, `peer_key_from_addr`.

## Differs / thin
- Verified metadata does not auto-`set_info` (returns `true` for caller).
- `dht_nodes` stored locally; client must feed `sources::Sources::add_dht_nodes`.
- Hash results must be drained by torrent/client loop (mark_complete / pend_all).
- Peer keys are addr-derived (stable); `next_peer_key` only counts registrations.

## Remains
- agy-2 must add the `pub(crate)` fields listed at the top of `torrent_impls.rs` and init them in `new` / `set_info` (call `init_book_and_glue`).
- Wire `note_peer_piece` from PeerConnHandler Have/Bitfield.
- Drain hasher results into book completion.

## Tests
- `tests/torrent_impls_peer_reader.rs` — trait method groups once fields compile.
- Helper unit tests for `peer_key_from_addr` in-module when crate builds.

# HANDOFF — wt-core Client (`client.rs`) — task_a92f237e38d7 / ctx_7c0bb47c5ea6

**Status: PAUSED** per coordinator (usage limits). Do not start the agy-2 unblock patches until resumed.

Worker terminal: `term_653cf843-3b3f-443a-8f24-249d12a2a65d`  
Owned: `src/client.rs`, `tests/client_*.rs`; thin `impl Client` delegates in `lib.rs` (coordinator option **c**).

## First next step (on resume)

1. **Unblock `cargo check -p wt-core`:** agy-2 breakage (not owned here):
   - `src/torrent.rs` ~L280 in `rescue_head_of_line`: uses undeclared `info` — need `info` from `self.info.read()` (piece_length / total_length) before `block_coord_from_req_index`.
   - `ReaderHost` for `TorrentInner`: comment in `reader.rs` says impl lives in `src/torrent_impls.rs`, but `lib.rs` does not `mod torrent_impls;` (file exists). Wire `pub mod torrent_impls;` **or** ask agy-2 — do **not** invent ReaderHost bodies without coordinator OK.
2. Then `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-core-client CARGO_BUILD_JOBS=3 cargo check -p wt-core --lib`.
3. Write `tests/client_*.rs`: two Clients localhost, TCP + uTP handshake (via `dial_peer` / `dial_peer_transport`), hybrid short-hash registry, clean `close()`, listen port.
4. `cargo fmt` + `clippy -p wt-core -- -D warnings` on owned files; append Client section to `PORT_NOTES.md`.
5. `worker_done` with honest acceptance.

## Done this dispatch

| Item | State |
|------|--------|
| `ClientInner::new` | Tokio 4 workers + blocking pool; PeerListener TCP+uTP; Sources bootstrap; nested `block_on` / `block_in_place` |
| Hybrid registry | `torrents` + `torrents_by_short` (v1 + v2 short) |
| `add_torrent_spec` | Create + **MergeSpec**-style merge (trackers, web seeds, info, DHT nodes); Sources register via adapters |
| Listeners / dial | `PeerListener::spawn_accept_loop` + SKEY/infohash lookup; `conn::dial_peer` + half-open + `Limiter`; legacy PeerConn attach after handshake |
| Limiters | dial / download / upload via `ratelimit::Limiter` |
| Hooks / stats | TCP hook, urgent steal duration, arrival state/stats, `ClientStats`, handshake callbacks |
| DHT | `add_dht_nodes` / `dht_contacts` via Sources + SharedDatagramAdapter |
| `lib.rs` Client | Thin same-signature delegates into `ClientInner` |
| Sources adapters | `TorrentSourcesAdapter` implements `PeerSink` + `TorrentAnnounceInfo` in `client.rs` |
| `tests/client_*.rs` | **Not written yet** (blocked on compile) |
| fmt/clippy / PORT_NOTES Client section | **Not done** |

## Files touched

- `engine-rs/crates/wt-core/src/client.rs` (rewrite)
- `engine-rs/crates/wt-core/src/lib.rs` (`impl Client` thin delegates only)
- `engine-rs/crates/wt-core/tests/CLIENT_HANDOFF.md` (this note)

## Build

`CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/wt-core-client` (keep &lt;4 GB).

## Risks / gaps

- PeerConn still legacy unbounded I/O after `conn::` handshake; full `PeerConnHandler`/`PeerTorrent` is agy-2.
- `TorrentInner.display_name` not interior-mutable → merge skips rename.
- MSE dial against own PeerListener; plain path available via `dial_peer_transport(..., require_plain: true)` for experiments.
- `Sources::start` failure is warn-only so constrained envs still get a Client.

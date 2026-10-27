# HANDOFF — peer_conn (grok-1 / term_850fca96)

Task `task_d1ce12daf22c` / dispatch `ctx_ec8c54c872b1`. Paused for planned break; resume after 6 pm.

## Done
- `PeerTorrent` trait + `WebtorPeerSnap` + `PeerKey` documented at top of `peer_conn.rs` (signature sent to coordinator earlier).
- `PeerConnHandler<T: PeerTorrent>: ConnHandler` — bitfield/have/have-all/have-none, choke/unchoke/interest, request/cancel upload via `choking::upload_allowed`, piece → storage + dirty + hash queue, reject/allowed-fast/suggest, port→DHT, LTEP + ut_metadata + ut_pex; BEP52 hash msgs accepted/no-op.
- Request loop: `compute_desired_requests` / `sort_requests_for_peer`; Webtor steal, faster-focus, slow-peer yield, pending/refill, peak modes, pipeline/`nominal_max`; HOL rescue + hedge when seek tuning enables them.
- Legacy `PeerConn` / `PeerConnInner` preserved for agy-2.
- `tests/peer_conn_handler_test.rs` — 8 tests with `FakeTorrent` + fake `ConnHandle` (all passed on a prior green wt-core build).
- `PORT_NOTES.md` peer_conn section; fmt + clippy clean on owned files (`-A clippy::too_many_arguments` only needed for agy-1 `conn/driver.rs`).
- Switched to shared `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-1`; deleted `/home/kar/.cache/wt-targets/wt-core`.

## Half-done / blocked
- Fresh in-crate `cargo test -p wt-core --test peer_conn_handler_test` blocked by **other owners**:
  - agy-2 `torrent.rs` ~280: `info` not in scope (E0425)
  - grok-8 `reader.rs`: `TorrentInner: ReaderHost` unbound (E0277)
  - workspace `wt-storage` Cargo.toml missing `benches/storage_bench.rs`
- `worker_done --outcome succeeded` already sent per coordinator ask option (c), noting the external breakage; coordinator will re-verify after 6 pm.

## Exact next steps on resume
1. Confirm wt-core compiles:  
   `cd engine-rs && CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-1 CARGO_BUILD_JOBS=3 cargo check -p wt-core`
2. Re-run:  
   `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-1 CARGO_BUILD_JOBS=3 cargo test -p wt-core --test peer_conn_handler_test`
3. Clippy owned surface:  
   `cargo clippy -p wt-core --test peer_conn_handler_test -- -D warnings -A clippy::too_many_arguments`
4. If crate still broken, add `tests/peer_conn_standalone/` `#[path]` harness (same pattern as `piece_state_standalone` / webtor-standalone).
5. agy-2: implement `PeerTorrent` on `TorrentInner`, spawn handler via `conn::spawn_connection_driver`.

## Open questions
- Whether BEP52 hash request handling should live here once piece layers exist, or stay no-op until grok-6/agy-2 wire it.
- Whether HOL/hedge should move to a torrent-wide `NudgeUrgentRequests` timer only (currently also inside `update_requests` when flags on).

## Verify command
```sh
cd /home/kar/orca/workspaces/torrent-player/rust-engine/engine-rs
CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-1 CARGO_BUILD_JOBS=3 \
  cargo test -p wt-core --test peer_conn_handler_test
```

# engine-go → engine-rs port map

Goal: a pure-Rust replacement for `engine-go` (and its patched
`engine-go/patches/torrent` = anacrolix/torrent v1.61.0 + Webtor patches)
that behaves the same as the Go engine and performs the same or better:
download throughput, seek-to-first-byte, CPU, RAM, battery. The Android app
(`android/`, `EngineHost.kt`) must work unchanged against the new `libengine.so`.

The Go engine is the reference. When in doubt, read the Go code and do what it does.
`engine-go/` stays untouched until the Rust engine passes parity.

## Crates and owners

| Crate | Owner | Ports (Go source) |
|---|---|---|
| `wt-types` | coordinator | shared data model + storage traits. Ask before changing. |
| `wt-meta` | grok-1 | `patches/torrent/{bencode,metainfo,merkle,types/infohash*}` — bencode, .torrent/info parse+encode, infohash v1/v2, magnets v1/v2/hybrid, BEP 47 padding, BEP 52 file tree + merkle + piece layers |
| `wt-wire` | agy-1 | `patches/torrent/{peer_protocol,mse}`, handshake.go, ltep.go, BEP 6 fast, BEP 9 ut_metadata, BEP 10, BEP 11 PEX, BEP 52 hash msgs, MSE/PE encryption, uTP transport (pure Rust), TCP/uTP listen+dial helpers |
| `wt-discovery` | grok-2 | `patches/torrent/{tracker,webtorrent,webseed}`, client-tracker-announcer.go, dht.go, bep40.go — HTTP/UDP trackers, announce scheduling, DHT (BEP 5) incl. contact save/restore, WebSocket WebTorrent tracker + WebRTC data channels, BEP 19 web seeds, BEP 40 priority |
| `wt-core` | agy-2 | rest of `patches/torrent` (client.go, torrent.go, peerconn.go, requesting.go, request-strategy, reader.go, piece.go, ...) + **all Webtor patches** (`webtor_urgent.go`, `webtor_arrival.go`, hybrid-magnet fix, outgoing TCP hook, CacheDialAddr) |
| `wt-storage` | agy-3 | storage.go, direct_storage.go, cache_store.go, cache_lru.go, checkpoint.go, extent*.go, pieces*.go, stream_ranges*.go, sparse_*.go, verification.go, restore_hash.go, zero_hash.go, and existing `engine-rust/src/{hash,ranges,lib}.rs` |
| `wt-policy` | grok-3 | adaptive_buffer.go, arrival_model.go, video_priority.go, playback_grid.go, playback_resume.go, next_preload.go, peer_exploration.go, seek_log.go, ghost_mode.go, ghost_pro.go, bypass.go, transfer_rates.go, direct_play_idle.go, claude_seek.go — as self-contained structs/functions with explicit inputs |
| `wt-parity` | grok-1 | parity + benchmark harness driving Go and Rust host engines over the control API |
| `wt-engine` | agy-4 | main.go (JNI exports, lib name `engine`), main_host.go (host binary), server.go, types.go, playback.go, preview.go, live_preview.go, trackers.go, tracker_status.go, network.go, connections.go, seeding.go, version.go; `build.sh` equivalent for Android |

Out of scope: `engine-go/cmd/discovery` (separate tool).

## Dependency order and API publishing

`wt-types` ← `wt-meta` ← `wt-wire`, `wt-discovery` ← `wt-core` ← `wt-engine`;
`wt-storage` and `wt-policy` depend only on `wt-types`/`wt-meta`.

Everyone starts at once. Upstream owners (meta, wire, discovery, storage,
policy) **publish their public API first** (types + signatures with
`unimplemented!()` bodies, compiling) and only then fill in bodies, so
downstream workers can code against it. `wt-core/src/lib.rs` already holds the
API `wt-engine` is written against; its owner implements it and may add to it,
but asks the coordinator before breaking a signature. Downstream workers read
upstream crates and never edit them; ask the coordinator for API changes.

## Rules for every worker

- Edit only your own crate directory (`engine-rs/crates/<yours>/`). Do not edit
  `engine-go/`, `android/`, `engine-rs/Cargo.toml` or other crates. Need a
  workspace change? Ask the coordinator.
- Add dependencies only in your crate's `Cargo.toml`. Pure Rust crates only
  (no C/C++ libraries besides libc/Android system libs; `ring` is acceptable).
  It must cross-compile to `aarch64-linux-android` with NDK 26.1.
- Build with your own target dir so builds do not block each other:
  `CARGO_TARGET_DIR=/tmp/wt-target-<crate> cargo test -p <crate>`.
  If `Cargo.lock` changes under you, just rerun cargo.
- Do **not** run any git command that writes (commit, add, stash, checkout,
  reset, restore, rebase, merge). The coordinator commits.
- Port the Go tests for your files to Rust tests. A ported behaviour is done
  when its Go test's Rust equivalent passes.
- Performance: no blocking calls on async tasks, bounded buffers and queues,
  no per-block allocation in the hot path where Go avoids it, `parking_lot`
  locks held briefly. Match Go's memory bounds.
- `cargo clippy -p <crate> -- -D warnings` and `cargo fmt` clean before done.
- Write a short `engine-rs/crates/<crate>/PORT_NOTES.md`: what is ported, what
  differs from Go and why, what is left.

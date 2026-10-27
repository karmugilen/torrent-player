# Resuming the Rust engine port

Paused at about 12:20 on 2026-10-27 for usage limits; resume after 18:00.

- Worktree: `/home/kar/orca/workspaces/torrent-player/rust-engine`, branch `karmugilen/rust-engine`.
- Orca run: `run_861102160a1b`. The coordinator was the Claude Code session in the main checkout.
- Progress report: `engine-rs/docs/PROGRESS.md`. Regenerate it with
  `rustc -O --edition 2021 tools/progress.rs -o /tmp/wt-progress && /tmp/wt-progress`, run from `engine-rs/`.
- Each worker left a `HANDOFF-<worker>.md` next to its files. Read these first.

## State at the pause

Done and verified (tests pass, committed):

| Crate / part | Commit(s) |
|---|---|
| wt-meta | c191e96 |
| wt-wire | 9721c3f |
| wt-discovery | fb0e093, 2648dd6 |
| wt-policy | 463dfe8, 882bc1e |
| wt-storage | b29e56c, Direct Play 5f688b0, tests 9eb5a54 |
| wt-parity (harness, Go baseline, API diff) | 518f33f, 6969bd9 |
| wt-core modules: strategy, choking, metadata, pex, ratelimit (+fix f9ce3cf), hashing, webseed_peer, piece_state, storage_adapter, webtor, sources, reader, conn | ef844c8, 76e488f, 0dc46ba, 7a327e6, 3c0d3fc, 1a08cbc, e6be3ec, b1f35f8 |
| wt-engine: JNI, engine-host, ctl_http, Android Gradle switch, stream server, preview, network, playback | ff2794a, 3c0d3fc, 9ccd54b, 07f3163 |
| Engine integration test suite (143 Go tests ported, most ignored until wired) | c841eac |

In progress when paused:

| Worker | Dispatch | Task | Spec |
|---|---|---|---|
| agy-2 | ctx_7e27ed8df81d | wt-core torrent.rs + lib.rs: central wiring of all modules; add TorrentInner fields for grok-9 | /tmp/wt-specs/core.md |
| grok-6 | ctx_7c0bb47c5ea6 | wt-core client.rs + impl Client block in lib.rs | /tmp/wt-specs/client.md |
| grok-1 | ctx_ec8c54c872b1 | wt-core peer_conn.rs (PeerTorrent trait, ConnHandler) | /tmp/wt-specs/peerconn.md |
| grok-9 | ctx_415e10a96dc0 | wt-core torrent_impls.rs (PeerTorrent + ReaderHost for TorrentInner) | /tmp/wt-specs/traitimpl.md |
| grok-5 | ctx_26e610b2d0b4 | wt-core end-to-end transfer tests | /tmp/wt-specs/e2e.md |
| grok-2 | ctx_a8266f5bfdbc | clippy-clean the finished wt-core modules | /tmp/wt-specs/lint.md |
| agy-4 | ctx_f6050d6ae8ee | wire EngineServer (server.rs) onto wt_core / wt_storage / wt_policy | /tmp/wt-specs/integrate.md |
| grok-4 | ctx_71810fd5ec87 | JSON byte-compat in types.rs (14 critical diffs vs Go) | /tmp/wt-specs/types.md |
| grok-7 | ctx_d2cb3cec4b34 | wt-storage missing accessors, un-ignore 16 tests | /tmp/wt-specs/accessors.md |
| agy-3 | ctx_3a3a09fb2647 | storage benchmarks vs Go | /tmp/wt-specs/storbench.md |

`/tmp` is cleared on reboot. Copies of all task specs are saved in `engine-rs/docs/specs/`.

## Critical path after resuming

1. agy-2 adds the TorrentInner fields grok-9 needs (list in `docs/specs/torrent_impls_fields.md`) and wires torrent.rs. grok-6, grok-1 and grok-9 finish client.rs, peer_conn.rs and torrent_impls.rs against it.
2. grok-5's e2e tests prove a real transfer: two Clients over TCP and uTP, and a magnet transfer.
3. agy-4 puts EngineServer on wt_core. Acceptance: two engine-host processes transfer a fixture torrent. grok-4 fixes the JSON diffs.
4. Un-ignore the 136 waiting engine integration tests.
5. Run `wt-parity run` (Rust) and `compare` against `results/go-all.json`. Rust must be the same or better than Go on throughput, TTFB, seek latency, RAM and CPU.
6. Run `./android/gradlew -p android :app:assembleDebug -Pwebtor.engine=rust` and test on a device.

## Rules that held during the run

- One crate or file set per worker; never edit others' files. Ask the coordinator.
- Build only with `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/<worker>` and `CARGO_BUILD_JOBS=3`. Never build under /tmp (RAM-backed tmpfs, which caused SIGBUS crashes).
- Keep at least 25 GB of disk free. The coordinator ran a governor (`/tmp/wt-specs/governor.sh`, copy in `docs/specs/`) that pauses builds below 10% free RAM.
- Keep your crate compiling at every checkpoint. The coordinator commits; workers run no git write commands.

# HANDOFF — wt-core e2e suite (task_a2384f1c0d4b / ctx_26e610b2d0b4)

**Status: PAUSED** per coordinator (2026-10-27). Do not wait on `wt-core/src`.

## Owned deliverables (written)

| Path | Role |
|------|------|
| `tests/e2e_support/mod.rs` | Memory fixtures, `seed_memory`, `add_client_peer`, greeting/patterned/multi-file |
| `tests/e2e_transfer.rs` | TCP/uTP greeting+patterned, magnet ut_metadata, drop/readd; MSE ignored |
| `tests/e2e_reader.rs` | sequential, seek→64KiB, readahead/responsive, cancel, drop-during-read |
| `tests/e2e_selection.rs` | piece Now, file select, pause/resume download, disallow upload |
| `tests/e2e_pex.rs` | 3-client relay, PEX-disabled transfer, magnet+x.pe peer |
| `tests/e2e_webtor.rs` | hybrid magnet, seek 64KiB, request_focus smoke |
| `tests/e2e_issues.rs` | add/drop, drop-before-info, max conns, listen port, initial state, trackers, short last piece |
| `tests/e2e_throughput.rs` | 1 MiB smoke + **ignored-heavy** 64 MiB TCP |
| `tests/E2E_TESTS.md` | Go→Rust map + counts + throughput TBD |
| `tests/E2E_HANDOFF.md` | this note |

## Results on last compiling build

- **Pass:** `e2e_issues` 8/8, `e2e_webtor` 3/3 (11 green).
- **Fail:** `e2e_pex` 0/3 (greeting stalls at 5/13 B; magnet download 0 after metadata).
- **Hang:** `e2e_transfer::transfer_greeting_tcp` (killed after long wait).
- **Not run after pex fail / src break:** reader, selection, throughput (incl. Go MiB/s compare).
- **Compile:** all seven `e2e_*` binaries built once; then `wt-core/src` broke (torrent.rs `info`, ReaderHost).

## Build note

Use **only** `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/grok-5`. Deleted `wt-targets/wt-core-e2e`.

## Resume checklist

1. `cargo check -p wt-core` green under `grok-5`.
2. `cargo test -p wt-core --test e2e_issues --test e2e_webtor --test e2e_reader --test e2e_selection --test e2e_transfer --test e2e_pex --test e2e_throughput -- --test-threads=1`
3. Route tiny-piece / post-metadata download bugs if still failing.
4. Run ignored 64 MiB throughput; update E2E_TESTS.md MiB/s (+ closest Go number).
5. `cargo fmt` on owned test files; worker_done with honest counts.

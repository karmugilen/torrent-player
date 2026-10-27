
## Shared context (read first)
- Repo worktree: /home/kar/orca/workspaces/torrent-player/rust-engine (branch karmugilen/rust-engine). Work ONLY here.
- Read engine-rs/docs/PORT_MAP.md fully: crate ownership, dependency order, rules. It is binding.
- Reference implementation: engine-go/ (Go engine) and engine-go/patches/torrent/ (patched anacrolix/torrent v1.61.0). Read the Go code and port its behaviour faithfully; the goal is the same or better performance than Go (throughput, seek latency, CPU, RAM).
- Shared types: engine-rs/crates/wt-types (coordinator-owned; ask before changing). Core API contract: engine-rs/crates/wt-core/src/lib.rs.
- Build/test with your own target dir: CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/<crate> CARGO_BUILD_JOBS=3 cargo test -p <crate> (run from engine-rs/). Never build under /tmp (RAM-backed tmpfs). Keep your target dir under 4 GB; fixtures at most 256 MiB under /home/kar/.cache/wt-tmp/<crate>, deleted after tests; one heavy test at a time; the disk must keep 25 GB free.
- NEVER run git write commands (commit/add/stash/checkout/reset/restore/merge/rebase). Never edit outside your crate directory.
- If you need an API from another crate that is not there yet, or need wt-types/workspace changes, use the orchestration ask command from your preamble to ask the coordinator; do not wait idle — continue with other parts meanwhile.
- Done means: your crate builds, `cargo clippy -p <crate> -- -D warnings` passes, `cargo fmt` applied, ported tests pass, and PORT_NOTES.md in your crate lists what is ported / differs / remains. Report honestly in worker_done (outcome failed if acceptance is not met).

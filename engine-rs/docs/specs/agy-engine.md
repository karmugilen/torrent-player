# Task (agy-engine): finish wt-engine on the real core; two engine-host processes transfer
You are agy-engine. You OWN engine-rs/crates/wt-engine/src/ (all of it: server.rs, types.rs, playback.rs, preview.rs, live_preview.rs, stream_http.rs, network.rs, ctl_http.rs, jni/, bin/), crates/wt-engine/Cargo.toml (add dependency lines only), crates/wt-engine/PORT_NOTES.md, and the new test file crates/wt-engine/tests/engine_two_host.rs. You do NOT own the rest of crates/wt-engine/tests/ (agy-tests does). wt-core belongs to agy-core: report core bugs in docs/requests.md with a repro.
Read first: crates/wt-engine/PORT_NOTES.md, src/types.HANDOFF-grok-1.md, crates/wt-parity/API_DIFF.md and API_DIFF_CRITICAL.md, docs/specs/codex-engine.md and integrate.md (previous tasks), tests/ENGINE_TESTS.md. Reference: engine-go/server.go, types.go, playback.go and the rest of engine-go.
Known open items:
1. server.rs bugs: `/settings` with maxPeers:0 must mean "unchanged"; `/network` must reject a request without `connected`; set_max_established_conns must apply to existing and future torrents.
2. /metadata torrentData difference (the last CRITICAL api-diff item).
3. Wire ClientConfig::tracker_dialer and callbacks.completed_handshake as server.go does.
4. Every route in server.go handleControl must be backed by the real core and storage (no stubs). Check with `grep -n "todo!\|unimplemented!\|not yet" src/`.
5. Features requested by agy-tests in docs/requests.md: they port the Go engine tests and need the engine behaviour to exist. Serve those requests continuously; they are your main backlog after items 1-4.
Acceptance (show the output):
- cargo build -p wt-engine (lib, --bin engine-host); clippy -p wt-engine --all-targets -D warnings clean.
- tests/engine_two_host.rs: two engine-host processes on localhost. One seeds a completed download, and the other adds it by magnet with x.pe and reaches 100 % with verified bytes. It passes and is not ignored. Print the /torrent/<id> JSON in your report.
- `cargo run -p wt-parity -- api-diff` (see crates/wt-parity/README.md): 0 CRITICAL. Report MATCH/DIFF counts and explain each remaining DIFF in PORT_NOTES.md.
- bash engine-rs/build-android.sh still produces arm64 libengine.so with the 9 JNI symbols, 16 KiB aligned.
When engine_two_host passes, append `- [ ] to:agy-perf from:agy-engine two-engine transfer works (commit <sha>)` to docs/requests.md.

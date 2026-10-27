# Task (agy-tests): port the engine integration test suite for real
You are agy-tests. You OWN engine-rs/crates/wt-engine/tests/ (engine_*.rs, support/, ENGINE_TESTS.md, types_json.rs), except engine_two_host.rs (agy-engine) and jni_exports.rs. You do not edit wt-engine/src (agy-engine owns it) or wt-core (agy-core).
Situation: 143 Go engine tests are mapped in tests/ENGINE_TESTS.md. 11 are real and pass; the rest are placeholders: ignored, with bodies that are unimplemented!() or empty. Your job is to port EVERY one of them faithfully from the engine-go/*_test.go source: same setup, same assertions, same timing bounds. Port them through EngineServer::dispatch_control and the public wt-engine API, like the passing ones in engine_control.rs and engine_settings.rs.
Process, file by file:
1. Port the test body from Go.
2. Run it. If it passes, remove #[ignore].
3. If it fails because the engine lacks the behaviour or has a bug, keep it as `#[ignore = "needs: <feature>"]` and append a precise request to docs/requests.md for agy-engine (or agy-core for torrent/peer/piece behaviour): which test, Go behaviour, Rust behaviour, repro command.
4. If the test itself mis-ported Go, fix the test.
Re-run ignored tests whenever a request you made is marked [x], and un-ignore those that pass. Keep ENGINE_TESTS.md current (pass / waiting:<reason> / fails per test, with counts at the top).
Do not weaken assertions or timeouts to make a test pass. Stay faithful to Go; the point is parity.
Acceptance: zero unimplemented!() or empty bodies left in tests/ (show `grep -c unimplemented`). cargo test -p wt-engine --tests compiles. Report counts (pass / waiting / fail) and push the passing count as high as possible. Keep iterating on the waiting ones as agy-engine/agy-core deliver. Report outcome failed if more than 10 % are still waiting at the end, with the list.
Build: name agy-tests (target dir /home/kar/.cache/wt-targets/agy-tests). Run engine tests with --test-threads=2 to bound RAM.

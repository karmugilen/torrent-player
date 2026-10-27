# Task (agy-core): finish wt-core and make every transfer path work
You are agy-core. You OWN ALL of engine-rs/crates/wt-core/ (src/ and tests/). You may also edit wt-wire, wt-meta and wt-discovery for bugs you find there, keeping their tests passing.
Read first: crates/wt-core/PORT_NOTES.md, tests/E2E_TESTS.md, tests/CLIENT_HANDOFF.md, every HANDOFF*.md in wt-core, docs/specs/codex-core.md and codex-client.md (the previous task), docs/specs/wake-interface.md, and the API notes at the top of each module.
Known open bugs (fix these first, root-cause each one):
1. Two-engine stall: a seeder restored at 100 % plus a leecher that got metadata over the wire via the magnet. The leecher stays at progress 0 with numPeers 0. Suspects: the connection is dropped at set_info and the 30 s backoff blocks a redial; bitfield/have-all is not re-applied after info arrives (no request update); the seeder does not unchoke or upload a restored-complete torrent; wants_conns returns 0 after info.
2. Tiny-piece stall: e2e_pex transfer_with_pex_disabled / three_client_relay_transfer (13 B greeting, piece_len 5) reaches bytes_completed=5 and then stalls. magnet_with_explicit_peer stays at 0 for 32 KiB.
3. uTP transfer on the new connection path. MSE (HeaderObfuscationPolicy on ClientConfig): un-ignore transfer_with_mse_encryption.
4. Clippy: unused infohashes/skey_matches, and items after the test module in torrent_impls.rs.
Acceptance (show the output):
- cargo test -p wt-core passes, including every tests/e2e_*.rs. Un-ignore and pass: TCP and uTP seeder->leecher transfers with verified data, magnet (ut_metadata) transfer, PEX relay, reader seek into missing data returning its first 64 KiB promptly, file selection, pause/resume, MSE.
- e2e_throughput 64 MiB loopback (release): MiB/s recorded in E2E_TESTS.md next to the Go library number (Go engine baseline: 19.5 MiB/s download through the engine; measure Go's library on the same loopback test if possible). Target: same as Go or better. Profile and fix if slower.
- clippy -p wt-core --all-targets -D warnings clean.
Then serve the requests addressed to agy-core in docs/requests.md (agy-engine will report core bugs it finds during the two-engine test) until agy-engine's two-engine test passes.

# Task (agy-tests, round 2): make the hollow ports faithful
You are agy-tests. The coordinator checked your round 1 (53962a0). The suite compiles and runs (87 pass, 0 fail, 92 ignored), but it was NOT accepted: 63 Rust tests have bodies under 40 % of the size of their Go original. engine-rs/docs/specs/weak-ports.txt lists each one with the Go and Rust non-comment line counts. Examples:
- test_local_peer_download_produces_playable_media: Go 91 lines, Rust 3. It only checks that GET /torrent returns 200.
- test_http_read_hands_over_the_first_block_at_once: no timing assertion at all.
- test_seek_http_comparison and test_seek_resume_comparison (Go 102/128 lines, Rust 7) run and "pass" without testing the seek behaviour.
A port is faithful when it has the same setup (swarm, local peers, fixture sizes, settings), the same actions, and EVERY Go assertion and timing bound, expressed through the Rust API. Shared helpers in tests/support/ are fine and encouraged; shortness is fine only when a helper does the same work. A test that cannot be expressed because the engine lacks an API stays #[ignore = "needs: <API>"], with the full body written against the API that the engine should have (behind a cfg or a small trait in support/ if needed so it compiles), plus a request line for agy-engine or agy-core.
Order: first the 6 that RUN (they give false passes), then the ignored ones by size gap.
Also: un-ignore each test that a `PASSES WITH --ignored` line in docs/requests.md names, and keep ENGINE_TESTS.md current with real counts.
Acceptance: re-run the size comparison (the coordinator's script is below). No test may be under 40 % of its Go size unless ENGINE_TESTS.md explains which helper does the remaining work. cargo test -p wt-engine compiles and has 0 failures. Report pass / waiting counts.
Size check script: python3 engine-rs/docs/specs/weak_ports.py (prints the list).

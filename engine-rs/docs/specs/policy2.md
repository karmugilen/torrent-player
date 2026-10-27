# Task: wt-policy follow-up — full test parity + clippy on all targets
Target: engine-rs/crates/wt-policy/ only (your previous task's crate; committed as 463dfe8).
Change:
1. `cargo clippy -p wt-policy --all-targets -- -D warnings` currently fails (int_plus_one in src/playback_grid.rs:184 test). Make it pass for ALL targets.
2. Go has 45 test functions in scope; Rust has 38. For each Go `func Test...` in adaptive_buffer_test.go, video_priority_test.go, playback_grid_test.go, playback_resume_test.go, next_preload_test.go, peer_exploration_test.go, ghost_mode_test.go, ghost_pro_test.go, bypass_test.go, transfer_rates_test.go, direct_play_idle_test.go (and claude_seek-related seek latency tests if any in engine-go/*_test.go), either port the policy assertions to a Rust test with a matching name, or list it in PORT_NOTES.md under "Tests left to wt-engine" with the reason (needs EngineServer/real peers). Add a table Go test -> Rust test | deferred.
3. Double-check numeric constants and thresholds against Go (grep every const/literal in the Go files you ported) and note any mismatch fixed.
Observable acceptance: cargo test -p wt-policy passes; clippy --all-targets -D warnings passes; fmt clean; PORT_NOTES.md has the complete Go->Rust test mapping table.

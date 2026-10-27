# Round 3 engine checkpoint

Commits: `18060a4` (preview/readers), `3c0c5d3` (shared playback/Ghost Pro windows), `8fadb87` (selection JSON/descriptors/original trackers), `69570d4` (verified resume persistence), and `77280dd` (UUID record identities and concurrent-add deduplication).

Needs ignores decreased from **70 to 38** across all integration files. Engine tests: **153 declared, 115 enabled, 38 ignored**. Enabled does not imply every original port covers all Go assertions. One additional enabled local-swarm test still fails exact-byte seek validation; the Claude slow/fast-seed counterpart now passes after the Grok fixes.

## Verification

- Library: 49 passed, including cached-contact Android startEngine regression and generation-safe resume expiry.
- Resume group: 5 instant-seek tests passed (2 still ignored), 3 playback-resume tests passed; original ignored assertions were preserved, with three new integration regressions supplying missing Go coverage.
- Ghost Pro 11/11, delivery 6/6, server 13/13, tracker pool 10/10, JSON 4/4, magnet-configure 2/2, cold-magnet restore 2/2, and two-host 1/1 passed.
- API diff after resume changes: **30 MATCH, 0 DIFF, 0 CRITICAL**.
- `cargo clippy -p wt-engine --all-targets --no-deps -- -D warnings`: passed.
- Broad suite (excluding the two previously identified core-stalled swarm tests) initially reached tracker-status and exposed a fixture mismatch: Rust used synthetic tracker-less metadata while Go uses `testdata/tiny.torrent`. Using the exact Go fixture preserves every assertion and tracker-status passes 11/11. UUID migration also corrected our earlier helpers to use returned record IDs for routes and info hashes for storage, preserving their assertions and time bounds.
- Broad rerun after the fixture corrections: **170 passed, 0 failed, 36 ignored, 4 filtered** (the two known core-stalled swarm tests plus their two ignored namesakes). Full dependency clippy remains blocked by unused imports/variables in core; engine-only all-target clippy passes. No core files were edited by this worker.
- No public real-swarm/peers_compare runs were performed.

## Every remaining ignored test

These are pending work, not claimed passes. A port defect is listed explicitly where its current body cannot establish the named behavior without weakening or replacing the Go assertion.

| Test | Why it remains waiting |
|---|---|
| `engine_direct_playback::test_direct_play_local_peer_seeks_beyond_ram_budget` | The duplicate port has no actual seed/producer; the active engine_swarm_playback regression still fails Direct Play seek byte integrity; no public swarm was run. |
| `engine_direct_playback::test_claude_seek_with_slow_and_fast_seed` | The duplicate port lacks the real Go slow/fast-seed fixture; its active engine_swarm_playback counterpart now passes, but this hollow duplicate remains ignored. |
| `engine_idle_buffer::test_playing_reader_keeps_window_and_peers_while_idle` | The current port opens/closes readers but omits Go idle-timer/window/peer assertions; keep waiting until those assertions and idle maintenance are ported faithfully. |
| `engine_idle_buffer::test_former_playhead_parks_once_it_is_no_longer_playing` | The current port opens/closes readers but omits Go idle-timer/window/peer assertions; keep waiting until those assertions and idle maintenance are ported faithfully. |
| `engine_instant_seek::test_reopening_the_same_file_does_not_replan_metadata` | The port calls set_startup_file but has no Go metadata-plan identity/accounting assertions; do not enable a hollow test. |
| `engine_instant_seek::test_http_read_hands_over_the_first_block_at_once` | The port reads an unverified range without delivering the first block, so it blocks; restore the Go controlled producer and first-block timing assertions. |
| `engine_metadata_cache::test_direct_play_saves_every_episodes_metadata_once` | Startup metadata save/commit behavior and the Go assertions are incomplete; background episode hint advancement still needs the requested storage on-startup-saved callback. |
| `engine_metadata_cache::test_direct_play_fetches_every_episodes_metadata_in_background` | Startup metadata save/commit behavior and the Go assertions are incomplete; background episode hint advancement still needs the requested storage on-startup-saved callback. |
| `engine_metadata_cache::test_validated_saved_metadata_replaces_partial_ram_piece` | The conditional assertion permits no verified bytes; the Go saved-generation reload, partial-buffer replacement and accounting checks are not yet ported. |
| `engine_metadata_cache::test_saved_metadata_commit_drops_partial_ram` | The current conditional assertion can silently skip the expected read and never checks RAM removal; port the unconditional Go commit/accounting assertions. |
| `engine_metadata_cache::test_saved_metadata_commit_clears_evicted_flag` | The current conditional assertion can silently skip the expected read and never checks eviction flags; port the Go eviction/commit assertions. |
| `engine_next_preload::test_direct_play_preloads_next_episode_near_the_end` | The current port calls /play without configuring storage, and next-episode startup/preload scheduling is not wired on the server. |
| `engine_playback::test_playback_reads_verified_bytes_without_waiting` | Fixture verifies only four of six pieces and then reads the entire file; port the Go NumPieces loop before judging verified delivery. |
| `engine_playback::test_playback_waits_for_verification_then_supports_seeking` | Fixture verifies only four of six pieces; Go verification wake/seek coverage needs all pieces plus its original bounded wait. |
| `engine_playback::test_playback_window_follows_seek_and_demotes_old_now` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_playback_scrub_has_immediate_full_readahead` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_playback_handles_share_priority_ownership` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_playback_seek_yields_startup_hints_to_target` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_playback_no_seek_hotspots_retained` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_playback_debounce_timer_quiet_expiration` | The playback debounce/quiet-expiry timer and full Go assertions still need porting; current test blocks on unverified bytes. |
| `engine_playback::test_playback_gated_open_phase_dual_now_and_moov_probe` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_playback_open_phase_exit_on_body_read` | The port performs blocking reads of unverified bytes without a producer and omits Go nonblocking window/priority assertions; full faithful regression remains. |
| `engine_playback::test_completed_and_streaming_http_ranges` | The port only calls /play; it omits actual HTTP Range requests and the Go completed/incomplete byte assertions. |
| `engine_playback::test_http_disconnect_cancels_missing_range_and_another_request_still_works` | The port only calls /play; it omits the HTTP connection abort, cancellation, and second-request assertions. |
| `engine_playback::test_local_peer_download_produces_playable_media` | The full local-peer payload/HTTP fixture is missing; the active Direct Play swarm regression still fails seek byte integrity. |
| `engine_playback::test_playback_boost_startup_window` | POST /play does not yet publish boost flags to the coordinator; the port also lacks Go startup span/priority assertions. |
| `engine_playback::test_http_playback_writes_a_seek_log_line` | The playback seek-log file path is not implemented; the port only calls /play and never inspects a seek log. |
| `engine_playback::test_unverified_reads_skip_the_start_and_end_of_the_file` | The port only calls /play and omits Go unverified head/tail producer and read assertions; startup hint semantics still need validation. |
| `engine_playback_active::test_only_the_latest_seek_position_is_fetched` | Current port reads verified data but omits Go demand ownership assertions; same-file latest-reader demotion remains unvalidated. |
| `engine_playback_active::test_blocked_read_at_an_earlier_spot_stops_and_stays_stopped` | A blocked old reader is joined before close/cancellation; production supersession must wake it and the Go bounded cancellation assertion must be retained. |
| `engine_playback_grid::test_seek_range_opens_the_window_at_the_range` | The port blocks on unverified bytes without a producer instead of using Go nonblocking offset/window helpers; opening-at-range and telemetry need their full Go assertions. |
| `engine_playback_grid::test_pieces_reports_playback_tiers_playhead_and_detail` | The port blocks on unverified bytes without a producer instead of using Go nonblocking offset/window helpers; opening-at-range and telemetry need their full Go assertions. |
| `engine_preview::test_preview_budget_enforced_across_seeks` | The port omits Go budget exhaustion setup and reads unavailable bytes; keep waiting for a faithful admitted-union/budget test. |
| `engine_preview_priority::test_preview_of_later_episode_is_not_starved_by_startup_hints` | Resting Direct Play wake/record-lock behavior and the Go demand assertions still need a faithful port; current reads have no payload producer. |
| `engine_preview_priority::test_preview_wakes_resting_direct_play` | Resting Direct Play wake/record-lock behavior and the Go demand assertions still need a faithful port; current reads have no payload producer. |
| `engine_preview_priority::test_reads_waking_resting_direct_play_do_not_deadlock_with_record_lock` | Current port never holds the record lock or performs a read, so it does not exercise the Go lock-order regression; faithful regression still needed. |
| `engine_seek_http::test_seek_http_comparison` | The Go multi-peer HTTP comparison harness and core peer/request reliability still need validation; the current port does not yet establish Go-equivalent coverage. |
| `engine_seek_http::test_seek_resume_comparison` | The Go multi-peer HTTP comparison harness and core peer/request reliability still need validation; the current port does not yet establish Go-equivalent coverage. |

## Remaining enabled failure

| Test | Why it remains blocked |
|---|---|
| `engine_swarm_playback::test_direct_play_local_peer_seeks_beyond_ram_budget` | Latest local run transfers beyond startup but seek at 25 MiB returns 4494 incorrect bytes, confined to a 4515-byte span from absolute byte 31600221 to 31604735; the exact-byte assertion is retained and evidence was escalated to the coordinator/grok-wire. |

## Remaining server implementation gaps

Startup episode metadata hint/commit advancement, next-episode preload scheduling, Direct Play resting-session wake/idle maintenance, complete /play validation/default-file selection and boost/buffer propagation, per-stream cache sizing/shrink behavior, and seek logging still need work. Several old playback/preview/HTTP ports also omit the original Go producer, nonblocking controls, or unconditional assertions; their ignores are deliberately retained rather than treating empty coverage as success.

## Reproduction commands and captured output

All commands run from `engine-rs` with `CARGO_TARGET_DIR=/home/kar/.cache/wt-targets/codex-engine CARGO_BUILD_JOBS=3 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 TMPDIR=/home/kar/.cache/wt-tmp/codex-engine`.

```text
cargo test -p wt-engine --lib
  test result: ok. 49 passed; 0 failed; 0 ignored
cargo test -p wt-engine --test engine_magnet_restore -- --ignored
  test test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata ... ok
cargo test -p wt-engine --test engine_two_host
  test result: ok. 1 passed; 0 failed; 0 ignored
cargo clippy -p wt-engine --all-targets --no-deps -- -D warnings
  Finished dev profile [unoptimized] target(s)
wt-parity api-diff --go /home/kar/.cache/wt-tmp/engine-go-host --rust /home/kar/.cache/wt-targets/codex-engine/debug/engine-host ...
  api-diff: 30 MATCH, 0 DIFF, 0 CRITICAL
```

Detailed command logs: `/home/kar/.cache/wt-tmp/codex-engine/round3-group4.log`, `round3-group5.log`, `round3-twohost-final.log`, `round3-clippy5.log`, `round3-api5.log`, `round3-clippy-full-final.log`, and `round3-final-suite.log`. The broad final-suite log sums to 170 passed, 36 ignored, 4 filtered, and zero failures. The two-host final log retains the original full fixture transfer and time bounds; no assertions were weakened.

Final localhost swarm rerun: `test_claude_seek_with_slow_and_fast_seed` passed (neutral 22.47 s, Claude 17.22 s), while `test_direct_play_local_peer_seeks_beyond_ram_budget` failed byte integrity at the 25 MiB seek. Log: `/home/kar/.cache/wt-tmp/codex-engine/round3-core-swarm-final.log`; panic arrays are large, so inspect clipped lines rather than dumping the file. Full default `cargo test -p wt-engine` is therefore still not green; the filtered broad suite is green.

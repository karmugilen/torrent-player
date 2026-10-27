# wt-policy port notes

Pure Rust port of `engine-go` playback/peer policy. No dependency on `wt-core`.
Time is injected (`Instant`) so tests never sleep. The only I/O surface is
`BypassStream` (tokio `AsyncRead` + `AsyncWrite` write-split wrapper).

## Ported

| Go source | Rust | Notes |
|---|---|---|
| `adaptive_buffer.go` | `AdaptiveBuffer` | Same EWMA / burst-aware / damping; floor `ADAPTIVE_BUFFER_MIN_BYTES` = `4<<20` |
| `arrival_model.go` | `ArrivalModelPersist` | Path + save gate only; neural net stays in wt-core |
| `video_priority.go` | `video_startup_hints`, `is_video_file_name` | Explicit file/context inputs |
| `playback_grid.go` | `split_urgent`, `playback_tiers`, `range_start`, … | Telemetry builders |
| `playback_resume.go` | `ResumeRange` | Timer expiry via `tick_expire(now)` |
| `next_preload.go` | `NextPreload` | Plan / should_start / activate / view |
| `peer_exploration.go` | `PeerExploration` | `PeerKey = u64` instead of `*PeerConn` |
| `ghost_mode.go` | `GhostDialController` | Dial-rate feedback |
| `ghost_pro.go` | `GhostProSwarm`, wanted/overlap helpers | Victim choice + piece sets |
| `bypass.go` | `BypassStream`, `BypassController`, `split_write_sync` | Async wrapper + sync helper |
| `transfer_rates.go` | `TransferRates` | Per-torrent EWMA |
| `direct_play_idle.go` | `IdleDirectPlay`, `playback_window_filling` | Rest / wake / drop-ready |
| `claude_seek.go` | `SeekLatencyStats`, drop-chokers, tuning | Stats + decisions (no dedicated Go unit tests) |
| `seek_log.go` | `SeekLog`, `StreamMeter` | Format/rotate decisions; engine does I/O |

## Differs from Go

- Peer identities are opaque `PeerKey` values assigned by the engine (no anacrolix types).
- Resume / idle / exploration use injected `Instant` instead of `time.AfterFunc` goroutines.
- `SeekLog` does not open files; it formats CSV and reports rotation.
- Arrival model JSON blob load/save is performed by the engine; this crate only decides path/when.
- Video mime detection uses a fixed extension table equivalent to Go's `mime` + torrent extras (no `/etc/mime.types`).
- Ghost Mode "clock went backwards" reset is unreachable with monotonic `Instant`; other reset paths are covered.

## Constants check (follow-up)

Grepped literals/consts in the ported Go files against `src/constants.rs` and
inline thresholds. All match; follow-up only named the adaptive floor explicitly:

| Symbol | Go | Rust |
|---|---|---|
| adaptive min window | `4<<20` | `ADAPTIVE_BUFFER_MIN_BYTES` |
| `playbackUrgentBytes` | 2 MiB | `PLAYBACK_URGENT_BYTES` |
| `playbackReadaheadBytes` | 24 MiB | `PLAYBACK_READAHEAD_BYTES` |
| `playbackScrubJumpBytes` | 2 MiB | `PLAYBACK_SCRUB_JUMP_BYTES` |
| `playbackScrubDebounce` | 250 ms | `PLAYBACK_SCRUB_DEBOUNCE` |
| `videoStartupHead/Tail` | 5 / 3 MiB | `VIDEO_STARTUP_*` |
| `playbackBoostHead/Tail/Readahead` | 16 / 8 / 48 MiB | `PLAYBACK_BOOST_*` |
| `directStartupHead/Tail` | 2 / 2 MiB | `DIRECT_STARTUP_*` |
| `playbackResumeTTL` | 90 s | `PLAYBACK_RESUME_TTL` |
| `resumePersistDelay` | 2 s | `RESUME_PERSIST_DELAY` |
| `resumeWindowMaxBytes` | 4 MiB | `RESUME_WINDOW_MAX_BYTES` |
| `nextPreloadAt` | 0.9 | `NEXT_PRELOAD_AT` |
| `defaultDialRate` / ceiling | 30 / 80 | `DEFAULT_DIAL_RATE` / `MAX_DIAL_RATE` |
| Ghost sample / gap / cooldown | 10 s / 30 s / 60 s | same in `GhostDialController` |
| Ghost keep / bail | 1.05× / 0.8× | same |
| Peer exploration sample / idle / rotate | 10 s / 90 s / 30 s | same |
| Ghost Pro sample / path idle / idle / rotate | 10 s / 15 s / 60 s / 20 s | same |
| `directPlayRestAfter` / `restPeerLimit` | 2 min / 80 | `DIRECT_PLAY_REST_AFTER` / `REST_PEER_LIMIT` |
| `bypassSplitFirstBytes` / delay | 8 / 2 ms | `BYPASS_SPLIT_*` |
| transfer EWMA / min interval | 0.6/0.4 / 0.5 s | same |
| `seekStatsWindow` | 20 | `SEEK_STATS_WINDOW` |
| `claudeSeekStealAfter` / nudge | 500 ms / 100 ms | `CLAUDE_SEEK_*` |
| `claudeSeekReadAttempts` | 3/6/12/24 s | `claude_seek_read_attempts()` |
| `chokeTimeout` | 5 s | `CHOKE_TIMEOUT` |
| `arrivalSaveEvery` | 1 min | `ARRIVAL_SAVE_EVERY` |
| `playbackDetailMax` | 2048 | `PLAYBACK_DETAIL_MAX` |
| `urgentNowExtraPieces` | 0 | `URGENT_NOW_EXTRA_PIECES` |
| `seekLogMaxBytes` | 2 MiB | `SEEK_LOG_MAX_BYTES` |

## Go → Rust test mapping

Scope: every `func Test…` in
`adaptive_buffer_test.go`, `video_priority_test.go`, `playback_grid_test.go`,
`playback_resume_test.go`, `next_preload_test.go`, `peer_exploration_test.go`,
`ghost_mode_test.go`, `ghost_pro_test.go`, `bypass_test.go`,
`transfer_rates_test.go`, `direct_play_idle_test.go` (45 tests), plus policy
parts of `idle_buffer_test.go` and claude-seek helpers (no dedicated Go unit
tests for `seekLatencyStats`).

| Go test | Rust test \| deferred |
|---|---|
| `TestAdaptiveBufferAddsHeadroomForVariableDemand` | `adaptive_buffer::test_adaptive_buffer_adds_headroom_for_variable_demand` |
| `TestBurstAwareBufferRetainsHeadroomAcrossLongerBursts` | `adaptive_buffer::test_burst_aware_buffer_retains_headroom_across_longer_bursts` |
| `TestIsVideoFileName` | `video_priority::test_is_video_file_name` |
| `TestVideoStartupHintsRaiseHeadAndTail` | `video_priority::test_video_startup_hints_raise_head_and_tail` |
| `TestVideoStartupHintsSkipNonVideoAndPaused` | `video_priority::test_video_startup_hints_skip_non_video_and_paused` (policy: skip non-video + paused clears hints; file-only / playlist advance deferred) |
| `TestVideoStartupHintsPlaybackCoexistence` | `video_priority::test_video_startup_hints_playback_coexistence` |
| `TestRangeStartParsesSingleRanges` | `playback_grid::test_range_start_parses_single_ranges` |
| `TestSeekRangeOpensTheWindowAtTheRange` | **deferred → wt-engine** — needs `openPlaybackAt` / real reader window |
| `TestPiecesReportsPlaybackTiersPlayheadAndDetail` | `playback_grid::test_pieces_reports_playback_tiers_playhead_and_detail` (tiers/playhead/detail builders; `/pieces` HTTP deferred) |
| `TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes` | `playback_resume::test_playback_close_reopen_preserves_resume_across_metadata_probes` (probe vs body + generation; OpenPlayback/persist deferred) |
| `TestOpenPlaybackReusesValidatedResumeWindow` | **deferred → wt-engine** — needs storage inject + OpenPlayback |
| `TestDirectPlayPreloadsNextEpisodeNearTheEnd` | `next_preload::test_direct_play_preloads_next_episode_near_the_end_plan_gate` + `_start_gate` (plan/start/ready); local-swarm warm deferred |
| `TestPeerExplorationProtectsUsefulUploadingAndNewPeers` | `peer_exploration::test_peer_exploration_protects_useful_uploading_and_new_peers` |
| `TestPeerExplorationLimitsChurnAndRequiresAlternatives` | `peer_exploration::test_peer_exploration_limits_churn_and_requires_alternatives` |
| `TestPeerExplorationSettingsAreIndependent` | **deferred → wt-engine** — `/settings` API |
| `TestPeerExplorationProtectsTheOnlyKnownSeed` | `peer_exploration::test_peer_exploration_protects_the_only_known_seed` |
| `TestPeerExplorationReleasesOneRealIdleConnection` | **deferred → wt-engine** — real TCP peer conns |
| `TestGhostModeKeepsMeasuredGainsAndLimitsDialing` | `ghost_mode::test_ghost_mode_keeps_measured_gains_and_limits_dialing` |
| `TestGhostModeRollsBackFailedProbesAndCoolsDown` | `ghost_mode::test_ghost_mode_rolls_back_failed_probes_and_cools_down` |
| `TestGhostModeDoesNotProbeBurstsOrZeroThroughput` | `ghost_mode::test_ghost_mode_does_not_probe_bursts_or_zero_throughput` |
| `TestGhostModeResetsForChangedConditions` | `ghost_mode::test_ghost_mode_resets_for_changed_conditions` (clock-backwards case N/A with `Instant`) |
| `TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments` | **deferred → wt-engine** — settings + dial limiter |
| `TestGhostModeSamplerHonorsPauseAndPayloadGate` | **deferred → wt-engine** — EngineServer sampler tick |
| `TestGhostModeCanToggleWhileSamplerRuns` | **deferred → wt-engine** — concurrent sampler |
| `TestGhostProPlaybackStartupSeekAndScaledWindows` | **deferred → wt-engine** — full playback fixture |
| `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance` | **deferred → wt-engine** — RAM cache budgets |
| `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle` | **deferred → wt-engine** — Direct Play seek fixture |
| `TestDirectPlayLargePiecesStayInsideTheCache` | **deferred → wt-engine** — cache sizing |
| `TestGhostProFirstVerifiedReadExpandsUrgency` | **deferred → wt-engine** — reader urgency expand |
| `TestGhostProPlaybackTelemetryMovesClearsAndToggles` | **deferred → wt-engine** — `/pieces` + settings |
| `TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces` | `ghost_pro::test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces` |
| `TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces` | `ghost_pro::test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces` |
| `TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders` | `ghost_pro::test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders` |
| `TestGhostProMutualExclusivityWithGhostModeAndPeerExploration` | **deferred → wt-engine** — settings exclusivity |
| `TestGhostProCanToggleWhileSamplerRuns` | **deferred → wt-engine** — concurrent sampler |
| `TestBypassDefaultsOff` | `bypass::test_bypass_defaults_off` |
| `TestBypassEnableActivatesTCPSplit` | `bypass::test_bypass_enable_activates_tcp_split` |
| `TestSplitConnFragmentsInitialWrite` | `bypass::test_split_conn_fragments_initial_write` |
| `TestSplitConnZeroProgressDoesNotSpin` | `bypass::test_split_conn_zero_progress_does_not_spin` |
| `TestBypassUsesPerClientHook` | `bypass::test_bypass_uses_per_client_hook` |
| `TestTorrentRatesAndETAUseTheirOwnSamples` | `transfer_rates::test_torrent_rates_and_eta_use_their_own_samples` (per-torrent independence; `/torrent` ETA view deferred) |
| `TestTransferRatesHandleIrregularSamplesPauseAndCounterReset` | `transfer_rates::test_transfer_rates_handle_irregular_samples_pause_and_counter_reset` |
| `TestPlaybackRestWaitsForReadaheadNotLookahead` | `idle_direct_play::test_playback_rest_waits_for_readahead_not_lookahead` |
| `TestRestDoesNotZeroTheConnectionCapAfterWake` | `idle_direct_play::test_rest_does_not_zero_the_connection_cap_after_wake_policy` (race gate); real peer-cap fixture deferred |
| `TestIdleDirectPlayRestsWakesAndBecomesDropReady` | `idle_direct_play::test_idle_direct_play_rests_wakes_and_becomes_drop_ready` |

### Tests left to wt-engine

Integration cases that need `EngineServer`, real peers, storage, or HTTP/settings:

- `TestSeekRangeOpensTheWindowAtTheRange`
- `TestOpenPlaybackReusesValidatedResumeWindow`
- `TestPeerExplorationSettingsAreIndependent`
- `TestPeerExplorationReleasesOneRealIdleConnection`
- `TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments`
- `TestGhostModeSamplerHonorsPauseAndPayloadGate`
- `TestGhostModeCanToggleWhileSamplerRuns`
- `TestGhostProPlaybackStartupSeekAndScaledWindows`
- `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance`
- `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle`
- `TestDirectPlayLargePiecesStayInsideTheCache`
- `TestGhostProFirstVerifiedReadExpandsUrgency`
- `TestGhostProPlaybackTelemetryMovesClearsAndToggles`
- `TestGhostProMutualExclusivityWithGhostModeAndPeerExploration`
- `TestGhostProCanToggleWhileSamplerRuns`
- Swarm / OpenPlayback halves of next-preload, resume persist, and idle rest peer-cap

### idle_buffer_test.go (policy parts)

| Go test | Rust test \| deferred |
|---|---|
| `TestPlayingReaderKeepsWindowAndPeersWhileIdle` | `idle_direct_play::test_playing_reader_keeps_window_and_peers_while_idle_buffer_filling` (buffer-filling gate); window park / `/play` wake deferred |
| `TestFormerPlayheadParksOnceItIsNoLongerPlaying` | **deferred → wt-engine** — playback coordinator idle timers |

### claude_seek / seek latency

No `func Test…` unit tests for `seekLatencyStats` / `dropChokersLoop` in engine-go.
Rust coverage: `seek_latency::seek_stats_median_and_window`,
`seek_latency::drop_chokers_respects_timeout_and_flag`. Seek swarm / HTTP seek
log tests stay in wt-engine.

## Remains

- Engine wiring: applying demand maps, dial hooks, rest peer address cache.
- Seek A/B experiment RNG lives in the engine (policy exposes `pick_seek_variant`).

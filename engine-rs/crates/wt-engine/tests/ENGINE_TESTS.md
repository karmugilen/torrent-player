# wt-engine integration test map

Go `engine-go/*_test.go` → Rust `tests/engine_*.rs` via `EngineServer::dispatch_control`.

Status values: `pass` (real test runs and passes), `waiting` (real test ignored pending a named feature), `placeholder` (ignored test body still needs porting), `fails` (real test runs but fails). Each placeholder has a feature reason in its `#[ignore]` attribute, but also requires a real test body.

| Go test | Rust test | Status |
|---|---|---|
| `TestOfflinePeerCountries` | `engine_connections::test_offline_peer_countries` | placeholder |
| `TestConnectionsExcludeCandidatesAndUnknownTorrent` | `engine_connections::test_connections_exclude_candidates_and_unknown_torrent` | placeholder |
| `TestConnectionsReportsRealTCPPeerAndClearsOnPause` | `engine_connections::test_connections_reports_real_tcp_peer_and_clears_on_pause` | placeholder |
| `TestNativeControlNeedsNoControlListener` | `engine_control::test_native_control_needs_no_control_listener` | pass |
| `TestChangeEventsAreRetainedAndBroadcast` | `engine_control::test_change_events_are_retained_and_broadcast` | pass |
| `TestDeliveryPreviewCanReuseVerifiedTail` | `engine_delivery_regression::test_delivery_preview_can_reuse_verified_tail` | placeholder |
| `TestDeliveryWarmCachePreservesExplicitPlayFromStart` | `engine_delivery_regression::test_delivery_warm_cache_preserves_explicit_play_from_start` | placeholder |
| `TestDeliveryConcurrentCacheClears` | `engine_delivery_regression::test_delivery_concurrent_cache_clears` | placeholder |
| `TestDeliveryExpiredPreviewReleasesSlot` | `engine_delivery_regression::test_delivery_expired_preview_releases_slot` | placeholder |
| `TestPreviewReleaseInvalidatesTemporaryCompletion` | `engine_delivery_regression::test_preview_release_invalidates_temporary_completion` | placeholder |
| `TestPreviewHonorsPayloadGate` | `engine_delivery_regression::test_preview_honors_payload_gate` | placeholder |
| `TestDirectPlayLocalPeerSeeksBeyondRAMBudget` | `engine_direct_playback::test_direct_play_local_peer_seeks_beyond_ram_budget` | placeholder |
| `TestIdleReaderParksAndResumesTheSameDescriptor` | `engine_direct_playback::test_idle_reader_parks_and_resumes_the_same_descriptor` | placeholder |
| `TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers` | `engine_direct_playback::test_stalled_read_retries_without_returning_zeros_or_dropping_peers` | placeholder |
| `TestDirectPlayCancelsWaitOnSeekAndClose` | `engine_direct_playback::test_direct_play_cancels_wait_on_seek_and_close` | placeholder |
| `TestClaudeSeekWithSlowAndFastSeed` | `engine_direct_playback::test_claude_seek_with_slow_and_fast_seed` | placeholder |
| `TestClaudeSeekSettingTogglesStealingAndResetsStats` | `engine_direct_playback::test_claude_seek_setting_toggles_stealing_and_resets_stats` | placeholder |
| `TestGhostModeKeepsMeasuredGainsAndLimitsDialing` | `engine_ghost_mode::test_ghost_mode_keeps_measured_gains_and_limits_dialing` | placeholder |
| `TestGhostModeRollsBackFailedProbesAndCoolsDown` | `engine_ghost_mode::test_ghost_mode_rolls_back_failed_probes_and_cools_down` | placeholder |
| `TestGhostModeDoesNotProbeBurstsOrZeroThroughput` | `engine_ghost_mode::test_ghost_mode_does_not_probe_bursts_or_zero_throughput` | placeholder |
| `TestGhostModeResetsForChangedConditions` | `engine_ghost_mode::test_ghost_mode_resets_for_changed_conditions` | placeholder |
| `TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments` | `engine_ghost_mode::test_ghost_mode_setting_restores_latest_base_and_preserves_other_experiments` | pass |
| `TestGhostModeSamplerHonorsPauseAndPayloadGate` | `engine_ghost_mode::test_ghost_mode_sampler_honors_pause_and_payload_gate` | placeholder |
| `TestGhostModeCanToggleWhileSamplerRuns` | `engine_ghost_mode::test_ghost_mode_can_toggle_while_sampler_runs` | placeholder |
| `TestGhostProPlaybackStartupSeekAndScaledWindows` | `engine_ghost_pro::test_ghost_pro_playback_startup_seek_and_scaled_windows` | placeholder |
| `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance` | `engine_ghost_pro::test_ghost_pro_instant_play_keeps_all_tiers_within_ram_allowance` | placeholder |
| `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle` | `engine_ghost_pro::test_direct_play_seek_is_smooth_forward_backward_and_middle` | placeholder |
| `TestDirectPlayLargePiecesStayInsideTheCache` | `engine_ghost_pro::test_direct_play_large_pieces_stay_inside_the_cache` | placeholder |
| `TestGhostProFirstVerifiedReadExpandsUrgency` | `engine_ghost_pro::test_ghost_pro_first_verified_read_expands_urgency` | placeholder |
| `TestGhostProPlaybackTelemetryMovesClearsAndToggles` | `engine_ghost_pro::test_ghost_pro_playback_telemetry_moves_clears_and_toggles` | placeholder |
| `TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces` | `engine_ghost_pro::test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces` | placeholder |
| `TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces` | `engine_ghost_pro::test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces` | placeholder |
| `TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders` | `engine_ghost_pro::test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders` | placeholder |
| `TestGhostProMutualExclusivityWithGhostModeAndPeerExploration` | `engine_ghost_pro::test_ghost_pro_mutual_exclusivity_with_ghost_mode_and_peer_exploration` | pass |
| `TestGhostProCanToggleWhileSamplerRuns` | `engine_ghost_pro::test_ghost_pro_can_toggle_while_sampler_runs` | placeholder |
| `TestAddHybridMagnetWithTrackersKeepsEngineAlive` | `engine_hybrid_magnet::test_add_hybrid_magnet_with_trackers_keeps_engine_alive` | pass |
| `TestPlayingReaderKeepsWindowAndPeersWhileIdle` | `engine_idle_buffer::test_playing_reader_keeps_window_and_peers_while_idle` | placeholder |
| `TestFormerPlayheadParksOnceItIsNoLongerPlaying` | `engine_idle_buffer::test_former_playhead_parks_once_it_is_no_longer_playing` | placeholder |
| `TestSeekServesDeliveredBlocksBeforeThePieceIsVerified` | `engine_instant_seek::test_seek_serves_delivered_blocks_before_the_piece_is_verified` | placeholder |
| `TestReopeningTheSameFileDoesNotReplanMetadata` | `engine_instant_seek::test_reopening_the_same_file_does_not_replan_metadata` | placeholder |
| `TestReinjectingResumeBytesDoesNotLeakCacheAccounting` | `engine_instant_seek::test_reinjecting_resume_bytes_does_not_leak_cache_accounting` | placeholder |
| `TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM` | `engine_instant_seek::test_resume_window_saves_once_seeking_settles_and_reopens_from_ram` | placeholder |
| `TestHTTPReadHandsOverTheFirstBlockAtOnce` | `engine_instant_seek::test_http_read_hands_over_the_first_block_at_once` | placeholder |
| `TestLivePreviewStateFollowsVerifiedPlaybackPosition` | `engine_live_preview::test_live_preview_state_follows_verified_playback_position` | placeholder |
| `TestPlaybackHandleSurvivesDownloadCompletion` | `engine_live_preview::test_playback_handle_survives_download_completion` | placeholder |
| `TestPlayingPreviewDoesNotDemandMissingPieces` | `engine_live_preview::test_playing_preview_does_not_demand_missing_pieces` | placeholder |
| `TestPendingMagnetCanResumeBeforeStorageIsConfigured` | `engine_magnet_restore::test_pending_magnet_can_resume_before_storage_is_configured` | placeholder |
| `TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata` | `engine_magnet_restore::test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata` | placeholder |
| `TestDirectPlaySavesEveryEpisodesMetadataOnce` | `engine_metadata_cache::test_direct_play_saves_every_episodes_metadata_once` | placeholder |
| `TestDirectPlayFetchesEveryEpisodesMetadataInBackground` | `engine_metadata_cache::test_direct_play_fetches_every_episodes_metadata_in_background` | placeholder |
| `TestValidatedSavedMetadataReplacesPartialRAMPiece` | `engine_metadata_cache::test_validated_saved_metadata_replaces_partial_ram_piece` | placeholder |
| `TestSavedMetadataCommitDropsPartialRAM` | `engine_metadata_cache::test_saved_metadata_commit_drops_partial_ram` | placeholder |
| `TestSavedMetadataCommitClearsEvictedFlag` | `engine_metadata_cache::test_saved_metadata_commit_clears_evicted_flag` | placeholder |
| `TestNetworkChangeIgnoresStaleAndDuplicateGenerations` | `engine_network::test_network_change_ignores_stale_and_duplicate_generations` | pass |
| `TestNetworkChangeRejectsInvalidBody` | `engine_network::test_network_change_rejects_invalid_body` | pass |
| `TestDirectPlayPreloadsNextEpisodeNearTheEnd` | `engine_next_preload::test_direct_play_preloads_next_episode_near_the_end` | placeholder |
| `TestPeerExplorationProtectsUsefulUploadingAndNewPeers` | `engine_peer_exploration::test_peer_exploration_protects_useful_uploading_and_new_peers` | placeholder |
| `TestPeerExplorationLimitsChurnAndRequiresAlternatives` | `engine_peer_exploration::test_peer_exploration_limits_churn_and_requires_alternatives` | placeholder |
| `TestPeerExplorationSettingsAreIndependent` | `engine_peer_exploration::test_peer_exploration_settings_are_independent` | pass |
| `TestPeerExplorationProtectsTheOnlyKnownSeed` | `engine_peer_exploration::test_peer_exploration_protects_the_only_known_seed` | placeholder |
| `TestPeerExplorationReleasesOneRealIdleConnection` | `engine_peer_exploration::test_peer_exploration_releases_one_real_idle_connection` | placeholder |
| `TestRangeStartParsesSingleRanges` | `engine_playback_grid::test_range_start_parses_single_ranges` | pass |
| `TestSeekRangeOpensTheWindowAtTheRange` | `engine_playback_grid::test_seek_range_opens_the_window_at_the_range` | placeholder |
| `TestPiecesReportsPlaybackTiersPlayheadAndDetail` | `engine_playback_grid::test_pieces_reports_playback_tiers_playhead_and_detail` | placeholder |
| `TestPlaybackReadsVerifiedBytesWithoutWaiting` | `engine_playback::test_playback_reads_verified_bytes_without_waiting` | placeholder |
| `TestPlaybackWaitsForVerificationThenSupportsSeeking` | `engine_playback::test_playback_waits_for_verification_then_supports_seeking` | placeholder |
| `TestPlaybackWindowFollowsSeekAndDemotesOldNow` | `engine_playback::test_playback_window_follows_seek_and_demotes_old_now` | placeholder |
| `TestPlaybackScrubHasImmediateFullReadahead` | `engine_playback::test_playback_scrub_has_immediate_full_readahead` | placeholder |
| `TestPlaybackHandlesSharePriorityOwnership` | `engine_playback::test_playback_handles_share_priority_ownership` | placeholder |
| `TestPlaybackSeekYieldsStartupHintsToTarget` | `engine_playback::test_playback_seek_yields_startup_hints_to_target` | placeholder |
| `TestPlaybackNoSeekHotspotsRetained` | `engine_playback::test_playback_no_seek_hotspots_retained` | placeholder |
| `TestPlaybackDebounceTimerQuietExpiration` | `engine_playback::test_playback_debounce_timer_quiet_expiration` | placeholder |
| `TestPlaybackGatedOpenPhaseDualNowAndMoovProbe` | `engine_playback::test_playback_gated_open_phase_dual_now_and_moov_probe` | placeholder |
| `TestPlaybackOpenPhaseExitOnBodyRead` | `engine_playback::test_playback_open_phase_exit_on_body_read` | placeholder |
| `TestFilePieceRangeAndOverlapHelpers` | `engine_playback::test_file_piece_range_and_overlap_helpers` | placeholder |
| `TestPlaybackCloseCancelsBlockedRead` | `engine_playback::test_playback_close_cancels_blocked_read` | placeholder |
| `TestCompletedAndStreamingHTTPRanges` | `engine_playback::test_completed_and_streaming_http_ranges` | placeholder |
| `TestHTTPDisconnectCancelsMissingRangeAndAnotherRequestStillWorks` | `engine_playback::test_http_disconnect_cancels_missing_range_and_another_request_still_works` | placeholder |
| `TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress` | `engine_playback::test_torrent_status_done_uses_verified_not_only_live_progress` | placeholder |
| `TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion` | `engine_playback::test_playback_rejects_unselected_files_and_unverified_completion` | placeholder |
| `TestLocalPeerDownloadProducesPlayableMedia` | `engine_playback::test_local_peer_download_produces_playable_media` | placeholder |
| `TestPlaybackBoostRequestDecodes` | `engine_playback::test_playback_boost_request_decodes` | placeholder |
| `TestPlaybackBoostStartupWindow` | `engine_playback::test_playback_boost_startup_window` | placeholder |
| `TestHTTPPlaybackWritesASeekLogLine` | `engine_playback::test_http_playback_writes_a_seek_log_line` | placeholder |
| `TestUnverifiedReadsSkipTheStartAndEndOfTheFile` | `engine_playback::test_unverified_reads_skip_the_start_and_end_of_the_file` | placeholder |
| `TestOnlyTheLatestSeekPositionIsFetched` | `engine_playback_active::test_only_the_latest_seek_position_is_fetched` | placeholder |
| `TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped` | `engine_playback_active::test_blocked_read_at_an_earlier_spot_stops_and_stays_stopped` | placeholder |
| `TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes` | `engine_playback_resume::test_playback_close_reopen_preserves_resume_across_metadata_probes` | placeholder |
| `TestOpenPlaybackReusesValidatedResumeWindow` | `engine_playback_resume::test_open_playback_reuses_validated_resume_window` | placeholder |
| `TestOpenPreviewReadsVerifiedBytesWithoutFocusChange` | `engine_preview::test_open_preview_reads_verified_bytes_without_focus_change` | placeholder |
| `TestPreviewRejectsSecondConcurrentJob` | `engine_preview::test_preview_rejects_second_concurrent_job` | pass |
| `TestPreviewTempForReadyDraftWithoutFullFile` | `engine_preview::test_preview_temp_for_ready_draft_without_full_file` | placeholder |
| `TestPreviewCloseIsIdempotentAndCancelsRead` | `engine_preview::test_preview_close_is_idempotent_and_cancels_read` | placeholder |
| `TestPauseReleasesPreviewDemand` | `engine_preview::test_pause_releases_preview_demand` | placeholder |
| `TestPreviewBudgetEnforcedAcrossSeeks` | `engine_preview::test_preview_budget_enforced_across_seeks` | placeholder |
| `TestPreviewSessionDeadlineBlocksUnlimitedReads` | `engine_preview::test_preview_session_deadline_blocks_unlimited_reads` | placeholder |
| `TestPreviewCloseJoinsBlockedRead` | `engine_preview::test_preview_close_joins_blocked_read` | placeholder |
| `TestPreviewConcurrentOpenAtomic` | `engine_preview::test_preview_concurrent_open_atomic` | placeholder |
| `TestPauseOtherTorrentLeavesPreview` | `engine_preview::test_pause_other_torrent_leaves_preview` | placeholder |
| `TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints` | `engine_preview_priority::test_preview_of_later_episode_is_not_starved_by_startup_hints` | placeholder |
| `TestPreviewWakesRestingDirectPlay` | `engine_preview_priority::test_preview_wakes_resting_direct_play` | placeholder |
| `TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock` | `engine_preview_priority::test_reads_waking_resting_direct_play_do_not_deadlock_with_record_lock` | placeholder |
| `TestSeekHTTPComparison` | `engine_seek_http::test_seek_http_comparison` | placeholder |
| `TestSeekResumeComparison` | `engine_seek_http::test_seek_resume_comparison` | placeholder |
| `TestStatusReportsConnectionsNotDiscoveredCandidates` | `engine_server::test_status_reports_connections_not_discovered_candidates` | placeholder |
| `TestAddMagnetBeforeMetadataReturnsRecord` | `engine_server::test_add_magnet_before_metadata_returns_record` | pass |
| `TestPauseIsReportedAndResumeRestoresSelection` | `engine_server::test_pause_is_reported_and_resume_restores_selection` | placeholder |
| `TestPayloadTransferSettingGatesBytesButPreservesManualPause` | `engine_server::test_payload_transfer_setting_gates_bytes_but_preserves_manual_pause` | placeholder |
| `TestPayloadGateKeepsPendingMagnetDiscoverable` | `engine_server::test_payload_gate_keeps_pending_magnet_discoverable` | placeholder |
| `TestPayloadGateRejectsIncompletePlaybackClearly` | `engine_server::test_payload_gate_rejects_incomplete_playback_clearly` | placeholder |
| `TestPieceTelemetryReportsSelectedPieces` | `engine_server::test_piece_telemetry_reports_selected_pieces` | placeholder |
| `TestAccumulateBucketPieceSelectedMixedCounts` | `engine_server::test_accumulate_bucket_piece_selected_mixed_counts` | placeholder |
| `TestPieceTelemetrySelectedVerifiedIgnoresExcluded` | `engine_server::test_piece_telemetry_selected_verified_ignores_excluded` | placeholder |
| `TestPieceTelemetrySharedBoundaryCountsAsSelected` | `engine_server::test_piece_telemetry_shared_boundary_counts_as_selected` | placeholder |
| `TestPieceTelemetryInclusiveRangesAndMaxBuckets` | `engine_server::test_piece_telemetry_inclusive_ranges_and_max_buckets` | placeholder |
| `TestSettingsApplyToExistingAndFutureTorrents` | `engine_settings::test_settings_apply_to_existing_and_future_torrents` | placeholder |
| `TestCacheShrinkPreservesDirtyDownloadBytes` | `engine_settings::test_cache_shrink_preserves_dirty_download_bytes` | placeholder |
| `TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded` | `engine_settings::test_cache_resize_keeps_direct_play_readable_and_preview_bounded` | placeholder |
| `TestSmallRAMSettingKeepsFourDirectPlayPieces` | `engine_settings::test_small_ram_setting_keeps_four_direct_play_pieces` | placeholder |
| `TestStatusFingerprintIgnoresPlaybackPriorityChurn` | `engine_telemetry::test_status_fingerprint_ignores_playback_priority_churn` | placeholder |
| `TestStatusFingerprintWakesOnVerificationAndRates` | `engine_telemetry::test_status_fingerprint_wakes_on_verification_and_rates` | placeholder |
| `TestVerifiedFileBytesCacheMatchesWalk` | `engine_telemetry::test_verified_file_bytes_cache_matches_walk` | placeholder |
| `TestBuildTrackerViewsMarksOriginalAndStatus` | `engine_tracker_status::test_build_tracker_views_marks_original_and_status` | placeholder |
| `TestGetTorrentIncludesTrackers` | `engine_tracker_status::test_get_torrent_includes_trackers` | placeholder |
| `TestAnnounceSlogUpdatesTrackerStatus` | `engine_tracker_status::test_announce_slog_updates_tracker_status` | placeholder |
| `TestAnnounceSlogLoggerContextAndGroupsAreRetained` | `engine_tracker_status::test_announce_slog_logger_context_and_groups_are_retained` | placeholder |
| `TestTrackerStatusDoesNotCreateGlobalFallback` | `engine_tracker_status::test_tracker_status_does_not_create_global_fallback` | placeholder |
| `TestTrackerStatusNormalizesIdentityAndURL` | `engine_tracker_status::test_tracker_status_normalizes_identity_and_url` | placeholder |
| `TestTrackerStatusForwardsBoundAttributesToDownstreamHandler` | `engine_tracker_status::test_tracker_status_forwards_bound_attributes_to_downstream_handler` | placeholder |
| `TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent` | `engine_tracker_status::test_status_updated_connection_error_uses_error_when_identity_is_present` | placeholder |
| `TestBuildTrackerViewsMarksAttachedTrackerSupplemental` | `engine_tracker_status::test_build_tracker_views_marks_attached_tracker_supplemental` | placeholder |
| `TestBuildTrackerViewsAttachedSupplementAppearsOnce` | `engine_tracker_status::test_build_tracker_views_attached_supplement_appears_once` | placeholder |
| `TestStatusUpdatedCallback` | `engine_tracker_status::test_status_updated_callback` | placeholder |
| `TestPublicTrackerParsing` | `engine_trackers::test_public_tracker_parsing` | placeholder |
| `TestTrackerCacheConditionalRefreshAndFailure` | `engine_trackers::test_tracker_cache_conditional_refresh_and_failure` | placeholder |
| `TestTrackerCacheCorruptionAndFallback` | `engine_trackers::test_tracker_cache_corruption_and_fallback` | placeholder |
| `TestTrackerFetchIsAsyncAndCancelledOnClose` | `engine_trackers::test_tracker_fetch_is_async_and_cancelled_on_close` | placeholder |
| `TestTrackerSupplementPublicPrivateAndMagnet` | `engine_trackers::test_tracker_supplement_public_private_and_magnet` | placeholder |
| `TestTrackerRefreshIsBoundedForExistingTorrents` | `engine_trackers::test_tracker_refresh_is_bounded_for_existing_torrents` | placeholder |
| `TestTrackerDiscoveredPeerSurvivesRefresh` | `engine_trackers::test_tracker_discovered_peer_survives_refresh` | placeholder |
| `TestManagedTrackersDoNotAccumulateAcrossRestore` | `engine_trackers::test_managed_trackers_do_not_accumulate_across_restore` | placeholder |
| `TestAddMagnetIgnoresDHTLabelAndKeepsTrackerURLs` | `engine_trackers::test_add_magnet_ignores_dht_label_and_keeps_tracker_ur_ls` | placeholder |
| `TestSuppliedTrackerValidationPreservesPrivatePasskeysAndTiers` | `engine_trackers::test_supplied_tracker_validation_preserves_private_passkeys_and_tiers` | placeholder |

## Deferred from wt-policy (also mapped here)

- `TestDirectPlayLargePiecesStayInsideTheCache`
- `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle`
- `TestFormerPlayheadParksOnceItIsNoLongerPlaying`
- `TestGhostModeCanToggleWhileSamplerRuns`
- `TestGhostModeSamplerHonorsPauseAndPayloadGate`
- `TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments`
- `TestGhostProCanToggleWhileSamplerRuns`
- `TestGhostProFirstVerifiedReadExpandsUrgency`
- `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance`
- `TestGhostProMutualExclusivityWithGhostModeAndPeerExploration`
- `TestGhostProPlaybackStartupSeekAndScaledWindows`
- `TestGhostProPlaybackTelemetryMovesClearsAndToggles`
- `TestOpenPlaybackReusesValidatedResumeWindow`
- `TestPeerExplorationReleasesOneRealIdleConnection`
- `TestPeerExplorationSettingsAreIndependent`
- `TestSeekRangeOpensTheWindowAtTheRange`

## Counts

- total mapped: 143
- real and passing: 11
- real but waiting: 0
- placeholders: 132
- fails: 0

## Compile note

`cargo test -p wt-engine --tests` passed on 2026-10-27, including JNI tests. Integration tests must run with `TMPDIR=/home/kar/.cache/wt-tmp/codex-5` to keep fixtures out of `/tmp`.


## Helper modules (stream / preview / network / playback)

- `src/stream_http.rs`: Go-faithful Range/HEAD/206 server (`/torrent/{id}/file/{index}`), unit + live Range test with fake `StreamSource`; `range_start` exported (powers `engine_playback_grid::test_range_start_parses_single_ranges`).
- `src/preview.rs`: budgeted `PreviewReader` + `admit_preview_range`.
- `src/live_preview.rs`: preview-state signature helpers.
- `src/network.rs`: generation gate + `NetworkDialer` reset/track.
- `src/playback.rs` + `playback_core.rs`: `PlaybackHost` / `ReaderBackend` / `PlaybackSession` / `PlaybackPriorityCoordinator` on wt-core + wt-policy; 6 lib unit tests with `FakeReaderBackend`. Integration `engine_playback*` remain ignored until agy-4 implements `PlaybackHost` and wires `open_playback` → `PlaybackReader::open_for_host`.

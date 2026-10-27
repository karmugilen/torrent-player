# wt-engine integration test map

Go `engine-go/*_test.go` → Rust `tests/engine_*.rs` via `EngineServer::dispatch_control`.

Status values: `pass` (runs and asserts), `ignored-waiting` (compiles, `#[ignore]` until feature lands), `fails` (runs but red).

| Go test | Rust test | Status |
|---|---|---|
| `TestOfflinePeerCountries` | `engine_connections::test_offline_peer_countries` | ignored-waiting |
| `TestConnectionsExcludeCandidatesAndUnknownTorrent` | `engine_connections::test_connections_exclude_candidates_and_unknown_torrent` | ignored-waiting |
| `TestConnectionsReportsRealTCPPeerAndClearsOnPause` | `engine_connections::test_connections_reports_real_tcp_peer_and_clears_on_pause` | ignored-waiting |
| `TestNativeControlNeedsNoControlListener` | `engine_control::test_native_control_needs_no_control_listener` | ignored-waiting |
| `TestChangeEventsAreRetainedAndBroadcast` | `engine_control::test_change_events_are_retained_and_broadcast` | ignored-waiting |
| `TestDeliveryPreviewCanReuseVerifiedTail` | `engine_delivery_regression::test_delivery_preview_can_reuse_verified_tail` | ignored-waiting |
| `TestDeliveryWarmCachePreservesExplicitPlayFromStart` | `engine_delivery_regression::test_delivery_warm_cache_preserves_explicit_play_from_start` | ignored-waiting |
| `TestDeliveryConcurrentCacheClears` | `engine_delivery_regression::test_delivery_concurrent_cache_clears` | ignored-waiting |
| `TestDeliveryExpiredPreviewReleasesSlot` | `engine_delivery_regression::test_delivery_expired_preview_releases_slot` | ignored-waiting |
| `TestPreviewReleaseInvalidatesTemporaryCompletion` | `engine_delivery_regression::test_preview_release_invalidates_temporary_completion` | ignored-waiting |
| `TestPreviewHonorsPayloadGate` | `engine_delivery_regression::test_preview_honors_payload_gate` | ignored-waiting |
| `TestDirectPlayLocalPeerSeeksBeyondRAMBudget` | `engine_direct_playback::test_direct_play_local_peer_seeks_beyond_ram_budget` | ignored-waiting |
| `TestIdleReaderParksAndResumesTheSameDescriptor` | `engine_direct_playback::test_idle_reader_parks_and_resumes_the_same_descriptor` | ignored-waiting |
| `TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers` | `engine_direct_playback::test_stalled_read_retries_without_returning_zeros_or_dropping_peers` | ignored-waiting |
| `TestDirectPlayCancelsWaitOnSeekAndClose` | `engine_direct_playback::test_direct_play_cancels_wait_on_seek_and_close` | ignored-waiting |
| `TestClaudeSeekWithSlowAndFastSeed` | `engine_direct_playback::test_claude_seek_with_slow_and_fast_seed` | ignored-waiting |
| `TestClaudeSeekSettingTogglesStealingAndResetsStats` | `engine_direct_playback::test_claude_seek_setting_toggles_stealing_and_resets_stats` | ignored-waiting |
| `TestGhostModeKeepsMeasuredGainsAndLimitsDialing` | `engine_ghost_mode::test_ghost_mode_keeps_measured_gains_and_limits_dialing` | ignored-waiting |
| `TestGhostModeRollsBackFailedProbesAndCoolsDown` | `engine_ghost_mode::test_ghost_mode_rolls_back_failed_probes_and_cools_down` | ignored-waiting |
| `TestGhostModeDoesNotProbeBurstsOrZeroThroughput` | `engine_ghost_mode::test_ghost_mode_does_not_probe_bursts_or_zero_throughput` | ignored-waiting |
| `TestGhostModeResetsForChangedConditions` | `engine_ghost_mode::test_ghost_mode_resets_for_changed_conditions` | ignored-waiting |
| `TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments` | `engine_ghost_mode::test_ghost_mode_setting_restores_latest_base_and_preserves_other_experiments` | pass |
| `TestGhostModeSamplerHonorsPauseAndPayloadGate` | `engine_ghost_mode::test_ghost_mode_sampler_honors_pause_and_payload_gate` | ignored-waiting |
| `TestGhostModeCanToggleWhileSamplerRuns` | `engine_ghost_mode::test_ghost_mode_can_toggle_while_sampler_runs` | ignored-waiting |
| `TestGhostProPlaybackStartupSeekAndScaledWindows` | `engine_ghost_pro::test_ghost_pro_playback_startup_seek_and_scaled_windows` | ignored-waiting |
| `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance` | `engine_ghost_pro::test_ghost_pro_instant_play_keeps_all_tiers_within_ram_allowance` | ignored-waiting |
| `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle` | `engine_ghost_pro::test_direct_play_seek_is_smooth_forward_backward_and_middle` | ignored-waiting |
| `TestDirectPlayLargePiecesStayInsideTheCache` | `engine_ghost_pro::test_direct_play_large_pieces_stay_inside_the_cache` | ignored-waiting |
| `TestGhostProFirstVerifiedReadExpandsUrgency` | `engine_ghost_pro::test_ghost_pro_first_verified_read_expands_urgency` | ignored-waiting |
| `TestGhostProPlaybackTelemetryMovesClearsAndToggles` | `engine_ghost_pro::test_ghost_pro_playback_telemetry_moves_clears_and_toggles` | ignored-waiting |
| `TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces` | `engine_ghost_pro::test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces` | ignored-waiting |
| `TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces` | `engine_ghost_pro::test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces` | ignored-waiting |
| `TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders` | `engine_ghost_pro::test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders` | ignored-waiting |
| `TestGhostProMutualExclusivityWithGhostModeAndPeerExploration` | `engine_ghost_pro::test_ghost_pro_mutual_exclusivity_with_ghost_mode_and_peer_exploration` | pass |
| `TestGhostProCanToggleWhileSamplerRuns` | `engine_ghost_pro::test_ghost_pro_can_toggle_while_sampler_runs` | ignored-waiting |
| `TestAddHybridMagnetWithTrackersKeepsEngineAlive` | `engine_hybrid_magnet::test_add_hybrid_magnet_with_trackers_keeps_engine_alive` | pass |
| `TestPlayingReaderKeepsWindowAndPeersWhileIdle` | `engine_idle_buffer::test_playing_reader_keeps_window_and_peers_while_idle` | ignored-waiting |
| `TestFormerPlayheadParksOnceItIsNoLongerPlaying` | `engine_idle_buffer::test_former_playhead_parks_once_it_is_no_longer_playing` | ignored-waiting |
| `TestSeekServesDeliveredBlocksBeforeThePieceIsVerified` | `engine_instant_seek::test_seek_serves_delivered_blocks_before_the_piece_is_verified` | ignored-waiting |
| `TestReopeningTheSameFileDoesNotReplanMetadata` | `engine_instant_seek::test_reopening_the_same_file_does_not_replan_metadata` | ignored-waiting |
| `TestReinjectingResumeBytesDoesNotLeakCacheAccounting` | `engine_instant_seek::test_reinjecting_resume_bytes_does_not_leak_cache_accounting` | ignored-waiting |
| `TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM` | `engine_instant_seek::test_resume_window_saves_once_seeking_settles_and_reopens_from_ram` | ignored-waiting |
| `TestHTTPReadHandsOverTheFirstBlockAtOnce` | `engine_instant_seek::test_http_read_hands_over_the_first_block_at_once` | ignored-waiting |
| `TestLivePreviewStateFollowsVerifiedPlaybackPosition` | `engine_live_preview::test_live_preview_state_follows_verified_playback_position` | ignored-waiting |
| `TestPlaybackHandleSurvivesDownloadCompletion` | `engine_live_preview::test_playback_handle_survives_download_completion` | ignored-waiting |
| `TestPlayingPreviewDoesNotDemandMissingPieces` | `engine_live_preview::test_playing_preview_does_not_demand_missing_pieces` | ignored-waiting |
| `TestPendingMagnetCanResumeBeforeStorageIsConfigured` | `engine_magnet_restore::test_pending_magnet_can_resume_before_storage_is_configured` | ignored-waiting |
| `TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata` | `engine_magnet_restore::test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata` | ignored-waiting |
| `TestDirectPlaySavesEveryEpisodesMetadataOnce` | `engine_metadata_cache::test_direct_play_saves_every_episodes_metadata_once` | ignored-waiting |
| `TestDirectPlayFetchesEveryEpisodesMetadataInBackground` | `engine_metadata_cache::test_direct_play_fetches_every_episodes_metadata_in_background` | ignored-waiting |
| `TestValidatedSavedMetadataReplacesPartialRAMPiece` | `engine_metadata_cache::test_validated_saved_metadata_replaces_partial_ram_piece` | ignored-waiting |
| `TestSavedMetadataCommitDropsPartialRAM` | `engine_metadata_cache::test_saved_metadata_commit_drops_partial_ram` | ignored-waiting |
| `TestSavedMetadataCommitClearsEvictedFlag` | `engine_metadata_cache::test_saved_metadata_commit_clears_evicted_flag` | ignored-waiting |
| `TestNetworkChangeIgnoresStaleAndDuplicateGenerations` | `engine_network::test_network_change_ignores_stale_and_duplicate_generations` | pass |
| `TestNetworkChangeRejectsInvalidBody` | `engine_network::test_network_change_rejects_invalid_body` | pass |
| `TestDirectPlayPreloadsNextEpisodeNearTheEnd` | `engine_next_preload::test_direct_play_preloads_next_episode_near_the_end` | ignored-waiting |
| `TestPeerExplorationProtectsUsefulUploadingAndNewPeers` | `engine_peer_exploration::test_peer_exploration_protects_useful_uploading_and_new_peers` | ignored-waiting |
| `TestPeerExplorationLimitsChurnAndRequiresAlternatives` | `engine_peer_exploration::test_peer_exploration_limits_churn_and_requires_alternatives` | ignored-waiting |
| `TestPeerExplorationSettingsAreIndependent` | `engine_peer_exploration::test_peer_exploration_settings_are_independent` | pass |
| `TestPeerExplorationProtectsTheOnlyKnownSeed` | `engine_peer_exploration::test_peer_exploration_protects_the_only_known_seed` | ignored-waiting |
| `TestPeerExplorationReleasesOneRealIdleConnection` | `engine_peer_exploration::test_peer_exploration_releases_one_real_idle_connection` | ignored-waiting |
| `TestRangeStartParsesSingleRanges` | `engine_playback_grid::test_range_start_parses_single_ranges` | ignored-waiting |
| `TestSeekRangeOpensTheWindowAtTheRange` | `engine_playback_grid::test_seek_range_opens_the_window_at_the_range` | ignored-waiting |
| `TestPiecesReportsPlaybackTiersPlayheadAndDetail` | `engine_playback_grid::test_pieces_reports_playback_tiers_playhead_and_detail` | ignored-waiting |
| `TestPlaybackReadsVerifiedBytesWithoutWaiting` | `engine_playback::test_playback_reads_verified_bytes_without_waiting` | ignored-waiting |
| `TestPlaybackWaitsForVerificationThenSupportsSeeking` | `engine_playback::test_playback_waits_for_verification_then_supports_seeking` | ignored-waiting |
| `TestPlaybackWindowFollowsSeekAndDemotesOldNow` | `engine_playback::test_playback_window_follows_seek_and_demotes_old_now` | ignored-waiting |
| `TestPlaybackScrubHasImmediateFullReadahead` | `engine_playback::test_playback_scrub_has_immediate_full_readahead` | ignored-waiting |
| `TestPlaybackHandlesSharePriorityOwnership` | `engine_playback::test_playback_handles_share_priority_ownership` | ignored-waiting |
| `TestPlaybackSeekYieldsStartupHintsToTarget` | `engine_playback::test_playback_seek_yields_startup_hints_to_target` | ignored-waiting |
| `TestPlaybackNoSeekHotspotsRetained` | `engine_playback::test_playback_no_seek_hotspots_retained` | ignored-waiting |
| `TestPlaybackDebounceTimerQuietExpiration` | `engine_playback::test_playback_debounce_timer_quiet_expiration` | ignored-waiting |
| `TestPlaybackGatedOpenPhaseDualNowAndMoovProbe` | `engine_playback::test_playback_gated_open_phase_dual_now_and_moov_probe` | ignored-waiting |
| `TestPlaybackOpenPhaseExitOnBodyRead` | `engine_playback::test_playback_open_phase_exit_on_body_read` | ignored-waiting |
| `TestFilePieceRangeAndOverlapHelpers` | `engine_playback::test_file_piece_range_and_overlap_helpers` | ignored-waiting |
| `TestPlaybackCloseCancelsBlockedRead` | `engine_playback::test_playback_close_cancels_blocked_read` | ignored-waiting |
| `TestCompletedAndStreamingHTTPRanges` | `engine_playback::test_completed_and_streaming_http_ranges` | ignored-waiting |
| `TestHTTPDisconnectCancelsMissingRangeAndAnotherRequestStillWorks` | `engine_playback::test_http_disconnect_cancels_missing_range_and_another_request_still_works` | ignored-waiting |
| `TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress` | `engine_playback::test_torrent_status_done_uses_verified_not_only_live_progress` | ignored-waiting |
| `TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion` | `engine_playback::test_playback_rejects_unselected_files_and_unverified_completion` | ignored-waiting |
| `TestLocalPeerDownloadProducesPlayableMedia` | `engine_playback::test_local_peer_download_produces_playable_media` | ignored-waiting |
| `TestPlaybackBoostRequestDecodes` | `engine_playback::test_playback_boost_request_decodes` | ignored-waiting |
| `TestPlaybackBoostStartupWindow` | `engine_playback::test_playback_boost_startup_window` | ignored-waiting |
| `TestHTTPPlaybackWritesASeekLogLine` | `engine_playback::test_http_playback_writes_a_seek_log_line` | ignored-waiting |
| `TestUnverifiedReadsSkipTheStartAndEndOfTheFile` | `engine_playback::test_unverified_reads_skip_the_start_and_end_of_the_file` | ignored-waiting |
| `TestOnlyTheLatestSeekPositionIsFetched` | `engine_playback_active::test_only_the_latest_seek_position_is_fetched` | ignored-waiting |
| `TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped` | `engine_playback_active::test_blocked_read_at_an_earlier_spot_stops_and_stays_stopped` | ignored-waiting |
| `TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes` | `engine_playback_resume::test_playback_close_reopen_preserves_resume_across_metadata_probes` | ignored-waiting |
| `TestOpenPlaybackReusesValidatedResumeWindow` | `engine_playback_resume::test_open_playback_reuses_validated_resume_window` | ignored-waiting |
| `TestOpenPreviewReadsVerifiedBytesWithoutFocusChange` | `engine_preview::test_open_preview_reads_verified_bytes_without_focus_change` | ignored-waiting |
| `TestPreviewRejectsSecondConcurrentJob` | `engine_preview::test_preview_rejects_second_concurrent_job` | pass |
| `TestPreviewTempForReadyDraftWithoutFullFile` | `engine_preview::test_preview_temp_for_ready_draft_without_full_file` | ignored-waiting |
| `TestPreviewCloseIsIdempotentAndCancelsRead` | `engine_preview::test_preview_close_is_idempotent_and_cancels_read` | ignored-waiting |
| `TestPauseReleasesPreviewDemand` | `engine_preview::test_pause_releases_preview_demand` | ignored-waiting |
| `TestPreviewBudgetEnforcedAcrossSeeks` | `engine_preview::test_preview_budget_enforced_across_seeks` | ignored-waiting |
| `TestPreviewSessionDeadlineBlocksUnlimitedReads` | `engine_preview::test_preview_session_deadline_blocks_unlimited_reads` | ignored-waiting |
| `TestPreviewCloseJoinsBlockedRead` | `engine_preview::test_preview_close_joins_blocked_read` | ignored-waiting |
| `TestPreviewConcurrentOpenAtomic` | `engine_preview::test_preview_concurrent_open_atomic` | ignored-waiting |
| `TestPauseOtherTorrentLeavesPreview` | `engine_preview::test_pause_other_torrent_leaves_preview` | ignored-waiting |
| `TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints` | `engine_preview_priority::test_preview_of_later_episode_is_not_starved_by_startup_hints` | ignored-waiting |
| `TestPreviewWakesRestingDirectPlay` | `engine_preview_priority::test_preview_wakes_resting_direct_play` | ignored-waiting |
| `TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock` | `engine_preview_priority::test_reads_waking_resting_direct_play_do_not_deadlock_with_record_lock` | ignored-waiting |
| `TestSeekHTTPComparison` | `engine_seek_http::test_seek_http_comparison` | ignored-waiting |
| `TestSeekResumeComparison` | `engine_seek_http::test_seek_resume_comparison` | ignored-waiting |
| `TestStatusReportsConnectionsNotDiscoveredCandidates` | `engine_server::test_status_reports_connections_not_discovered_candidates` | ignored-waiting |
| `TestAddMagnetBeforeMetadataReturnsRecord` | `engine_server::test_add_magnet_before_metadata_returns_record` | pass |
| `TestPauseIsReportedAndResumeRestoresSelection` | `engine_server::test_pause_is_reported_and_resume_restores_selection` | ignored-waiting |
| `TestPayloadTransferSettingGatesBytesButPreservesManualPause` | `engine_server::test_payload_transfer_setting_gates_bytes_but_preserves_manual_pause` | ignored-waiting |
| `TestPayloadGateKeepsPendingMagnetDiscoverable` | `engine_server::test_payload_gate_keeps_pending_magnet_discoverable` | ignored-waiting |
| `TestPayloadGateRejectsIncompletePlaybackClearly` | `engine_server::test_payload_gate_rejects_incomplete_playback_clearly` | ignored-waiting |
| `TestPieceTelemetryReportsSelectedPieces` | `engine_server::test_piece_telemetry_reports_selected_pieces` | ignored-waiting |
| `TestAccumulateBucketPieceSelectedMixedCounts` | `engine_server::test_accumulate_bucket_piece_selected_mixed_counts` | ignored-waiting |
| `TestPieceTelemetrySelectedVerifiedIgnoresExcluded` | `engine_server::test_piece_telemetry_selected_verified_ignores_excluded` | ignored-waiting |
| `TestPieceTelemetrySharedBoundaryCountsAsSelected` | `engine_server::test_piece_telemetry_shared_boundary_counts_as_selected` | ignored-waiting |
| `TestPieceTelemetryInclusiveRangesAndMaxBuckets` | `engine_server::test_piece_telemetry_inclusive_ranges_and_max_buckets` | ignored-waiting |
| `TestSettingsApplyToExistingAndFutureTorrents` | `engine_settings::test_settings_apply_to_existing_and_future_torrents` | ignored-waiting |
| `TestCacheShrinkPreservesDirtyDownloadBytes` | `engine_settings::test_cache_shrink_preserves_dirty_download_bytes` | ignored-waiting |
| `TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded` | `engine_settings::test_cache_resize_keeps_direct_play_readable_and_preview_bounded` | ignored-waiting |
| `TestSmallRAMSettingKeepsFourDirectPlayPieces` | `engine_settings::test_small_ram_setting_keeps_four_direct_play_pieces` | ignored-waiting |
| `TestStatusFingerprintIgnoresPlaybackPriorityChurn` | `engine_telemetry::test_status_fingerprint_ignores_playback_priority_churn` | ignored-waiting |
| `TestStatusFingerprintWakesOnVerificationAndRates` | `engine_telemetry::test_status_fingerprint_wakes_on_verification_and_rates` | ignored-waiting |
| `TestVerifiedFileBytesCacheMatchesWalk` | `engine_telemetry::test_verified_file_bytes_cache_matches_walk` | ignored-waiting |
| `TestBuildTrackerViewsMarksOriginalAndStatus` | `engine_tracker_status::test_build_tracker_views_marks_original_and_status` | ignored-waiting |
| `TestGetTorrentIncludesTrackers` | `engine_tracker_status::test_get_torrent_includes_trackers` | ignored-waiting |
| `TestAnnounceSlogUpdatesTrackerStatus` | `engine_tracker_status::test_announce_slog_updates_tracker_status` | ignored-waiting |
| `TestAnnounceSlogLoggerContextAndGroupsAreRetained` | `engine_tracker_status::test_announce_slog_logger_context_and_groups_are_retained` | ignored-waiting |
| `TestTrackerStatusDoesNotCreateGlobalFallback` | `engine_tracker_status::test_tracker_status_does_not_create_global_fallback` | ignored-waiting |
| `TestTrackerStatusNormalizesIdentityAndURL` | `engine_tracker_status::test_tracker_status_normalizes_identity_and_url` | ignored-waiting |
| `TestTrackerStatusForwardsBoundAttributesToDownstreamHandler` | `engine_tracker_status::test_tracker_status_forwards_bound_attributes_to_downstream_handler` | ignored-waiting |
| `TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent` | `engine_tracker_status::test_status_updated_connection_error_uses_error_when_identity_is_present` | ignored-waiting |
| `TestBuildTrackerViewsMarksAttachedTrackerSupplemental` | `engine_tracker_status::test_build_tracker_views_marks_attached_tracker_supplemental` | ignored-waiting |
| `TestBuildTrackerViewsAttachedSupplementAppearsOnce` | `engine_tracker_status::test_build_tracker_views_attached_supplement_appears_once` | ignored-waiting |
| `TestStatusUpdatedCallback` | `engine_tracker_status::test_status_updated_callback` | ignored-waiting |
| `TestPublicTrackerParsing` | `engine_trackers::test_public_tracker_parsing` | ignored-waiting |
| `TestTrackerCacheConditionalRefreshAndFailure` | `engine_trackers::test_tracker_cache_conditional_refresh_and_failure` | ignored-waiting |
| `TestTrackerCacheCorruptionAndFallback` | `engine_trackers::test_tracker_cache_corruption_and_fallback` | ignored-waiting |
| `TestTrackerFetchIsAsyncAndCancelledOnClose` | `engine_trackers::test_tracker_fetch_is_async_and_cancelled_on_close` | ignored-waiting |
| `TestTrackerSupplementPublicPrivateAndMagnet` | `engine_trackers::test_tracker_supplement_public_private_and_magnet` | ignored-waiting |
| `TestTrackerRefreshIsBoundedForExistingTorrents` | `engine_trackers::test_tracker_refresh_is_bounded_for_existing_torrents` | ignored-waiting |
| `TestTrackerDiscoveredPeerSurvivesRefresh` | `engine_trackers::test_tracker_discovered_peer_survives_refresh` | ignored-waiting |
| `TestManagedTrackersDoNotAccumulateAcrossRestore` | `engine_trackers::test_managed_trackers_do_not_accumulate_across_restore` | ignored-waiting |
| `TestAddMagnetIgnoresDHTLabelAndKeepsTrackerURLs` | `engine_trackers::test_add_magnet_ignores_dht_label_and_keeps_tracker_ur_ls` | ignored-waiting |
| `TestSuppliedTrackerValidationPreservesPrivatePasskeysAndTiers` | `engine_trackers::test_supplied_tracker_validation_preserves_private_passkeys_and_tiers` | ignored-waiting |

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
- pass: 8
- ignored-waiting: 135
- fails: 0

## Compile note

Owned suite (`tests/engine_*.rs` + `tests/support/`) compiles and runs via
`cargo test -p wt-engine --test engine_<name>`. Full `cargo test -p wt-engine --tests`
is blocked by `tests/jni_exports.rs` (grok-4 owned) until its `wt_engine::` imports
become `engine::`.


## Helper modules (stream / preview / network)

- `src/stream_http.rs`: Go-faithful Range/HEAD/206 server (`/torrent/{id}/file/{index}`), unit + live Range test with fake `StreamSource`.
- `src/preview.rs`: budgeted `PreviewReader` + `admit_preview_range`.
- `src/live_preview.rs`: preview-state signature helpers.
- `src/network.rs`: generation gate + `NetworkDialer` reset/track.

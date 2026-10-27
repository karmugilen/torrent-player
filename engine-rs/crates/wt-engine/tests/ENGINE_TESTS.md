# wt-engine integration test map

Engine integration tests declared: 148 | Enabled: 92 | Waiting: 56 | Known core-blocked failures: 2

Round 3 group 1: 12 additional preview tests passed unchanged and were enabled; all integration `needs:` ignores, including JSON tests, decreased from 70 to 58. Library tests: 47 pass. The two existing local swarm playback failures remain with Grok.

Go `engine-go/*_test.go` → Rust `tests/engine_*.rs` via `EngineServer::dispatch_control`.

Status values: `pass` (real test runs and passes), `waiting: <reason>` (real test ignored pending a named feature), `placeholder` (ignored test body still needs porting), `fails` (real test runs but fails).

| Go test | Rust test | Status |
|---|---|---|
| `TestOfflinePeerCountries` | `engine_connections::test_offline_peer_countries` | pass |
| `TestConnectionsExcludeCandidatesAndUnknownTorrent` | `engine_connections::test_connections_exclude_candidates_and_unknown_torrent` | pass |
| `TestConnectionsReportsRealTCPPeerAndClearsOnPause` | `engine_connections::test_connections_reports_real_tcp_peer_and_clears_on_pause` | pass |
| `TestNativeControlNeedsNoControlListener` | `engine_control::test_native_control_needs_no_control_listener` | pass |
| `TestChangeEventsAreRetainedAndBroadcast` | `engine_control::test_change_events_are_retained_and_broadcast` | pass |
| `TestDeliveryPreviewCanReuseVerifiedTail` | `engine_delivery_regression::test_delivery_preview_can_reuse_verified_tail` | pass |
| `TestDeliveryWarmCachePreservesExplicitPlayFromStart` | `engine_delivery_regression::test_delivery_warm_cache_preserves_explicit_play_from_start` | pass |
| `TestDeliveryConcurrentCacheClears` | `engine_delivery_regression::test_delivery_concurrent_cache_clears` | pass |
| `TestDeliveryExpiredPreviewReleasesSlot` | `engine_delivery_regression::test_delivery_expired_preview_releases_slot` | pass |
| `TestPreviewReleaseInvalidatesTemporaryCompletion` | `engine_delivery_regression::test_preview_release_invalidates_temporary_completion` | pass |
| `TestPreviewHonorsPayloadGate` | `engine_delivery_regression::test_preview_honors_payload_gate` | pass |
| `TestDirectPlayLocalPeerSeeksBeyondRAMBudget` | `engine_direct_playback::test_direct_play_local_peer_seeks_beyond_ram_budget` | waiting: multi-peer local swarm transfer with seeking |
| `TestIdleReaderParksAndResumesTheSameDescriptor` | `engine_direct_playback::test_idle_reader_parks_and_resumes_the_same_descriptor` | waiting: PlaybackReader park_idle and in_flight tracking |
| `TestStalledReadRetriesWithoutReturningZerosOrDroppingPeers` | `engine_direct_playback::test_stalled_read_retries_without_returning_zeros_or_dropping_peers` | pass |
| `TestDirectPlayCancelsWaitOnSeekAndClose` | `engine_direct_playback::test_direct_play_cancels_wait_on_seek_and_close` | waiting: cancellable wait on direct play read and cancel on seek/close |
| `TestClaudeSeekWithSlowAndFastSeed` | `engine_direct_playback::test_claude_seek_with_slow_and_fast_seed` | waiting: multi-peer slow and fast seed transfer |
| `TestClaudeSeekSettingTogglesStealingAndResetsStats` | `engine_direct_playback::test_claude_seek_setting_toggles_stealing_and_resets_stats` | pass |
| `TestGhostModeKeepsMeasuredGainsAndLimitsDialing` | `engine_ghost_mode::test_ghost_mode_keeps_measured_gains_and_limits_dialing` | pass |
| `TestGhostModeRollsBackFailedProbesAndCoolsDown` | `engine_ghost_mode::test_ghost_mode_rolls_back_failed_probes_and_cools_down` | pass |
| `TestGhostModeDoesNotProbeBurstsOrZeroThroughput` | `engine_ghost_mode::test_ghost_mode_does_not_probe_bursts_or_zero_throughput` | pass |
| `TestGhostModeResetsForChangedConditions` | `engine_ghost_mode::test_ghost_mode_resets_for_changed_conditions` | pass |
| `TestGhostModeSettingRestoresLatestBaseAndPreservesOtherExperiments` | `engine_ghost_mode::test_ghost_mode_setting_restores_latest_base_and_preserves_other_experiments` | pass |
| `TestGhostModeSamplerHonorsPauseAndPayloadGate` | `engine_ghost_mode::test_ghost_mode_sampler_honors_pause_and_payload_gate` | pass |
| `TestGhostModeCanToggleWhileSamplerRuns` | `engine_ghost_mode::test_ghost_mode_can_toggle_while_sampler_runs` | pass |
| `TestGhostProPlaybackStartupSeekAndScaledWindows` | `engine_ghost_pro::test_ghost_pro_playback_startup_seek_and_scaled_windows` | waiting: Ghost Pro scaled windows and priority tiers |
| `TestGhostProInstantPlayKeepsAllTiersWithinRAMAllowance` | `engine_ghost_pro::test_ghost_pro_instant_play_keeps_all_tiers_within_ram_allowance` | pass |
| `TestDirectPlaySeekIsSmoothForwardBackwardAndMiddle` | `engine_ghost_pro::test_direct_play_seek_is_smooth_forward_backward_and_middle` | waiting: Direct Play seek windows and shared cache |
| `TestDirectPlayLargePiecesStayInsideTheCache` | `engine_ghost_pro::test_direct_play_large_pieces_stay_inside_the_cache` | pass |
| `TestGhostProFirstVerifiedReadExpandsUrgency` | `engine_ghost_pro::test_ghost_pro_first_verified_read_expands_urgency` | pass |
| `TestGhostProPlaybackTelemetryMovesClearsAndToggles` | `engine_ghost_pro::test_ghost_pro_playback_telemetry_moves_clears_and_toggles` | waiting: Ghost Pro playback telemetry moves and clears |
| `TestContiguousWantedPiecesExcludesReaderGapsAndCompletedPieces` | `engine_ghost_pro::test_contiguous_wanted_pieces_excludes_reader_gaps_and_completed_pieces` | pass |
| `TestGhostProPrioritizesIdlePeersWithoutPlaybackPathPieces` | `engine_ghost_pro::test_ghost_pro_prioritizes_idle_peers_without_playback_path_pieces` | pass |
| `TestGhostProChoosesZeroOverlapIdleVictimAndProtectsSeeders` | `engine_ghost_pro::test_ghost_pro_chooses_zero_overlap_idle_victim_and_protects_seeders` | pass |
| `TestGhostProMutualExclusivityWithGhostModeAndPeerExploration` | `engine_ghost_pro::test_ghost_pro_mutual_exclusivity_with_ghost_mode_and_peer_exploration` | pass |
| `TestGhostProCanToggleWhileSamplerRuns` | `engine_ghost_pro::test_ghost_pro_can_toggle_while_sampler_runs` | pass |
| `TestAddHybridMagnetWithTrackersKeepsEngineAlive` | `engine_hybrid_magnet::test_add_hybrid_magnet_with_trackers_keeps_engine_alive` | pass |
| `TestPlayingReaderKeepsWindowAndPeersWhileIdle` | `engine_idle_buffer::test_playing_reader_keeps_window_and_peers_while_idle` | waiting: playing reader idle window management |
| `TestFormerPlayheadParksOnceItIsNoLongerPlaying` | `engine_idle_buffer::test_former_playhead_parks_once_it_is_no_longer_playing` | waiting: former playhead parking when no longer playing |
| `TestSeekServesDeliveredBlocksBeforeThePieceIsVerified` | `engine_instant_seek::test_seek_serves_delivered_blocks_before_the_piece_is_verified` | waiting: delivered blocks stream without full piece verification |
| `TestReopeningTheSameFileDoesNotReplanMetadata` | `engine_instant_seek::test_reopening_the_same_file_does_not_replan_metadata` | waiting: reopening file avoids replanning metadata |
| `TestReinjectingResumeBytesDoesNotLeakCacheAccounting` | `engine_instant_seek::test_reinjecting_resume_bytes_does_not_leak_cache_accounting` | waiting: re-injecting resume bytes cache accounting |
| `TestResumeWindowSavesOnceSeekingSettlesAndReopensFromRAM` | `engine_instant_seek::test_resume_window_saves_once_seeking_settles_and_reopens_from_ram` | waiting: resume window scheduled persistence and reopen from RAM |
| `TestHTTPReadHandsOverTheFirstBlockAtOnce` | `engine_instant_seek::test_http_read_hands_over_the_first_block_at_once` | waiting: HTTP read hands over first block immediately (ReadSome) |
| `TestLivePreviewStateFollowsVerifiedPlaybackPosition` | `engine_live_preview::test_live_preview_state_follows_verified_playback_position` | pass |
| `TestPlaybackHandleSurvivesDownloadCompletion` | `engine_live_preview::test_playback_handle_survives_download_completion` | pass |
| `TestPlayingPreviewDoesNotDemandMissingPieces` | `engine_live_preview::test_playing_preview_does_not_demand_missing_pieces` | pass |
| `TestPendingMagnetCanResumeBeforeStorageIsConfigured` | `engine_magnet_restore::test_pending_magnet_can_resume_before_storage_is_configured` | pass |
| `TestColdRestoredPendingMagnetDiscoversTrackerPeerAndMetadata` | `engine_magnet_restore::test_cold_restored_pending_magnet_discovers_tracker_peer_and_metadata` | waiting: cold restored magnet metadata via local tracker |
| `TestDirectPlaySavesEveryEpisodesMetadataOnce` | `engine_metadata_cache::test_direct_play_saves_every_episodes_metadata_once` | waiting: startup metadata saving across episodes |
| `TestDirectPlayFetchesEveryEpisodesMetadataInBackground` | `engine_metadata_cache::test_direct_play_fetches_every_episodes_metadata_in_background` | waiting: background metadata fetch for every episode in swarm |
| `TestValidatedSavedMetadataReplacesPartialRAMPiece` | `engine_metadata_cache::test_validated_saved_metadata_replaces_partial_ram_piece` | waiting: validated saved metadata replacing partial RAM piece |
| `TestSavedMetadataCommitDropsPartialRAM` | `engine_metadata_cache::test_saved_metadata_commit_drops_partial_ram` | waiting: startup metadata commit drops partial RAM buffer |
| `TestSavedMetadataCommitClearsEvictedFlag` | `engine_metadata_cache::test_saved_metadata_commit_clears_evicted_flag` | waiting: saved metadata commit clears evicted lost flag |
| `TestNetworkChangeIgnoresStaleAndDuplicateGenerations` | `engine_network::test_network_change_ignores_stale_and_duplicate_generations` | pass |
| `TestNetworkChangeRejectsInvalidBody` | `engine_network::test_network_change_rejects_invalid_body` | pass |
| `TestDirectPlayPreloadsNextEpisodeNearTheEnd` | `engine_next_preload::test_direct_play_preloads_next_episode_near_the_end` | waiting: next episode preload near end of playback |
| `TestPeerExplorationProtectsUsefulUploadingAndNewPeers` | `engine_peer_exploration::test_peer_exploration_protects_useful_uploading_and_new_peers` | pass |
| `TestPeerExplorationLimitsChurnAndRequiresAlternatives` | `engine_peer_exploration::test_peer_exploration_limits_churn_and_requires_alternatives` | pass |
| `TestPeerExplorationSettingsAreIndependent` | `engine_peer_exploration::test_peer_exploration_settings_are_independent` | pass |
| `TestPeerExplorationProtectsTheOnlyKnownSeed` | `engine_peer_exploration::test_peer_exploration_protects_the_only_known_seed` | pass |
| `TestPeerExplorationReleasesOneRealIdleConnection` | `engine_peer_exploration::test_peer_exploration_releases_one_real_idle_connection` | pass |
| `TestRangeStartParsesSingleRanges` | `engine_playback_grid::test_range_start_parses_single_ranges` | pass |
| `TestSeekRangeOpensTheWindowAtTheRange` | `engine_playback_grid::test_seek_range_opens_the_window_at_the_range` | waiting: open_playback_at with seek offset opening window at range |
| `TestPiecesReportsPlaybackTiersPlayheadAndDetail` | `engine_playback_grid::test_pieces_reports_playback_tiers_playhead_and_detail` | waiting: GET /pieces reporting playback tiers, playhead and detail |
| `TestPlaybackReadsVerifiedBytesWithoutWaiting` | `engine_playback::test_playback_reads_verified_bytes_without_waiting` | waiting: playback reader read_at verified range without blocking |
| `TestPlaybackWaitsForVerificationThenSupportsSeeking` | `engine_playback::test_playback_waits_for_verification_then_supports_seeking` | waiting: playback reader verification wake and seek support |
| `TestPlaybackWindowFollowsSeekAndDemotesOldNow` | `engine_playback::test_playback_window_follows_seek_and_demotes_old_now` | waiting: playback window follows seek and demotes old Now priority |
| `TestPlaybackScrubHasImmediateFullReadahead` | `engine_playback::test_playback_scrub_has_immediate_full_readahead` | waiting: playback scrub immediate full readahead |
| `TestPlaybackHandlesSharePriorityOwnership` | `engine_playback::test_playback_handles_share_priority_ownership` | waiting: playback multi-handle shared priority ownership |
| `TestPlaybackSeekYieldsStartupHintsToTarget` | `engine_playback::test_playback_seek_yields_startup_hints_to_target` | waiting: playback seek yields startup hints to target |
| `TestPlaybackNoSeekHotspotsRetained` | `engine_playback::test_playback_no_seek_hotspots_retained` | waiting: playback no seek hotspots retained |
| `TestPlaybackDebounceTimerQuietExpiration` | `engine_playback::test_playback_debounce_timer_quiet_expiration` | waiting: playback debounce timer quiet expiration |
| `TestPlaybackGatedOpenPhaseDualNowAndMoovProbe` | `engine_playback::test_playback_gated_open_phase_dual_now_and_moov_probe` | waiting: playback gated open phase dual now and moov probe |
| `TestPlaybackOpenPhaseExitOnBodyRead` | `engine_playback::test_playback_open_phase_exit_on_body_read` | waiting: playback open phase exit on body read |
| `TestFilePieceRangeAndOverlapHelpers` | `engine_playback::test_file_piece_range_and_overlap_helpers` | pass |
| `TestPlaybackCloseCancelsBlockedRead` | `engine_playback::test_playback_close_cancels_blocked_read` | waiting: playback close cancels blocked read |
| `TestCompletedAndStreamingHTTPRanges` | `engine_playback::test_completed_and_streaming_http_ranges` | waiting: HTTP streaming range server requests |
| `TestHTTPDisconnectCancelsMissingRangeAndAnotherRequestStillWorks` | `engine_playback::test_http_disconnect_cancels_missing_range_and_another_request_still_works` | waiting: HTTP disconnect cancels missing range request |
| `TestTorrentStatusDoneUsesVerifiedNotOnlyLiveProgress` | `engine_playback::test_torrent_status_done_uses_verified_not_only_live_progress` | waiting: torrent status Done requires verified bytes completion |
| `TestPlaybackRejectsUnselectedFilesAndUnverifiedCompletion` | `engine_playback::test_playback_rejects_unselected_files_and_unverified_completion` | waiting: playback rejects unselected files and unverified completion |
| `TestLocalPeerDownloadProducesPlayableMedia` | `engine_playback::test_local_peer_download_produces_playable_media` | waiting: two-engine local swarm transfer produces playable media |
| `TestPlaybackBoostRequestDecodes` | `engine_playback::test_playback_boost_request_decodes` | pass |
| `TestPlaybackBoostStartupWindow` | `engine_playback::test_playback_boost_startup_window` | waiting: playback boost startup window expansion |
| `TestHTTPPlaybackWritesASeekLogLine` | `engine_playback::test_http_playback_writes_a_seek_log_line` | waiting: HTTP playback seek log file entry |
| `TestUnverifiedReadsSkipTheStartAndEndOfTheFile` | `engine_playback::test_unverified_reads_skip_the_start_and_end_of_the_file` | waiting: unverified reads skip head and tail hints |
| `TestOnlyTheLatestSeekPositionIsFetched` | `engine_playback_active::test_only_the_latest_seek_position_is_fetched` | waiting: active playback coordinator demoting older seek positions |
| `TestBlockedReadAtAnEarlierSpotStopsAndStaysStopped` | `engine_playback_active::test_blocked_read_at_an_earlier_spot_stops_and_stays_stopped` | waiting: blocked read at earlier spot superseded by new seek |
| `TestPlaybackCloseReopenPreservesResumeAcrossMetadataProbes` | `engine_playback_resume::test_playback_close_reopen_preserves_resume_across_metadata_probes` | waiting: playback close/reopen preserving resume across metadata probes |
| `TestOpenPlaybackReusesValidatedResumeWindow` | `engine_playback_resume::test_open_playback_reuses_validated_resume_window` | waiting: open_playback reusing validated resume window |
| `TestOpenPreviewReadsVerifiedBytesWithoutFocusChange` | `engine_preview::test_open_preview_reads_verified_bytes_without_focus_change` | pass |
| `TestPreviewRejectsSecondConcurrentJob` | `engine_preview::test_preview_rejects_second_concurrent_job` | pass |
| `TestPreviewTempForReadyDraftWithoutFullFile` | `engine_preview::test_preview_temp_for_ready_draft_without_full_file` | pass |
| `TestPreviewCloseIsIdempotentAndCancelsRead` | `engine_preview::test_preview_close_is_idempotent_and_cancels_read` | pass |
| `TestPauseReleasesPreviewDemand` | `engine_preview::test_pause_releases_preview_demand` | pass |
| `TestPreviewBudgetEnforcedAcrossSeeks` | `engine_preview::test_preview_budget_enforced_across_seeks` | waiting: preview budget enforcement across seeks outside admitted union |
| `TestPreviewSessionDeadlineBlocksUnlimitedReads` | `engine_preview::test_preview_session_deadline_blocks_unlimited_reads` | pass |
| `TestPreviewCloseJoinsBlockedRead` | `engine_preview::test_preview_close_joins_blocked_read` | pass |
| `TestPreviewConcurrentOpenAtomic` | `engine_preview::test_preview_concurrent_open_atomic` | pass |
| `TestPauseOtherTorrentLeavesPreview` | `engine_preview::test_pause_other_torrent_leaves_preview` | pass |
| `TestPreviewOfLaterEpisodeIsNotStarvedByStartupHints` | `engine_preview_priority::test_preview_of_later_episode_is_not_starved_by_startup_hints` | waiting: preview not starved by startup hints in multi-episode swarm |
| `TestPreviewWakesRestingDirectPlay` | `engine_preview_priority::test_preview_wakes_resting_direct_play` | waiting: preview wakes resting direct play |
| `TestReadsWakingRestingDirectPlayDoNotDeadlockWithRecordLock` | `engine_preview_priority::test_reads_waking_resting_direct_play_do_not_deadlock_with_record_lock` | waiting: preview read and close deadlock protection with record lock |
| `TestSeekHTTPComparison` | `engine_seek_http::test_seek_http_comparison` | waiting: multi-peer simulated seek swarm HTTP comparison |
| `TestSeekResumeComparison` | `engine_seek_http::test_seek_resume_comparison` | waiting: seek tuning comparison across multi-peer swarm |
| `TestStatusReportsConnectionsNotDiscoveredCandidates` | `engine_server::test_status_reports_connections_not_discovered_candidates` | pass |
| `TestAddMagnetBeforeMetadataReturnsRecord` | `engine_server::test_add_magnet_before_metadata_returns_record` | pass |
| `TestPauseIsReportedAndResumeRestoresSelection` | `engine_server::test_pause_is_reported_and_resume_restores_selection` | pass |
| `TestPayloadTransferSettingGatesBytesButPreservesManualPause` | `engine_server::test_payload_transfer_setting_gates_bytes_but_preserves_manual_pause` | pass |
| `TestPayloadGateKeepsPendingMagnetDiscoverable` | `engine_server::test_payload_gate_keeps_pending_magnet_discoverable` | pass |
| `TestPayloadGateRejectsIncompletePlaybackClearly` | `engine_server::test_payload_gate_rejects_incomplete_playback_clearly` | pass |
| `TestPieceTelemetryReportsSelectedPieces` | `engine_server::test_piece_telemetry_reports_selected_pieces` | pass |
| `TestAccumulateBucketPieceSelectedMixedCounts` | `engine_server::test_accumulate_bucket_piece_selected_mixed_counts` | pass |
| `TestPieceTelemetrySelectedVerifiedIgnoresExcluded` | `engine_server::test_piece_telemetry_selected_verified_ignores_excluded` | waiting: piece telemetry selected verified ignores excluded |
| `TestPieceTelemetrySharedBoundaryCountsAsSelected` | `engine_server::test_piece_telemetry_shared_boundary_counts_as_selected` | waiting: piece telemetry shared boundary counts as selected |
| `TestPieceTelemetryInclusiveRangesAndMaxBuckets` | `engine_server::test_piece_telemetry_inclusive_ranges_and_max_buckets` | pass |
| `TestSettingsApplyToExistingAndFutureTorrents` | `engine_settings::test_settings_apply_to_existing_and_future_torrents` | pass |
| `TestCacheShrinkPreservesDirtyDownloadBytes` | `engine_settings::test_cache_shrink_preserves_dirty_download_bytes` | pass |
| `TestCacheResizeKeepsDirectPlayReadableAndPreviewBounded` | `engine_settings::test_cache_resize_keeps_direct_play_readable_and_preview_bounded` | pass |
| `TestSmallRAMSettingKeepsFourDirectPlayPieces` | `engine_settings::test_small_ram_setting_keeps_four_direct_play_pieces` | pass |
| `TestStatusFingerprintIgnoresPlaybackPriorityChurn` | `engine_telemetry::test_status_fingerprint_ignores_playback_priority_churn` | pass |
| `TestStatusFingerprintWakesOnVerificationAndRates` | `engine_telemetry::test_status_fingerprint_wakes_on_verification_and_rates` | pass |
| `TestVerifiedFileBytesCacheMatchesWalk` | `engine_telemetry::test_verified_file_bytes_cache_matches_walk` | pass |
| `TestBuildTrackerViewsMarksOriginalAndStatus` | `engine_tracker_status::test_build_tracker_views_marks_original_and_status` | pass |
| `TestGetTorrentIncludesTrackers` | `engine_tracker_status::test_get_torrent_includes_trackers` | pass |
| `TestAnnounceSlogUpdatesTrackerStatus` | `engine_tracker_status::test_announce_slog_updates_tracker_status` | pass |
| `TestAnnounceSlogLoggerContextAndGroupsAreRetained` | `engine_tracker_status::test_announce_slog_logger_context_and_groups_are_retained` | pass |
| `TestTrackerStatusDoesNotCreateGlobalFallback` | `engine_tracker_status::test_tracker_status_does_not_create_global_fallback` | pass |
| `TestTrackerStatusNormalizesIdentityAndURL` | `engine_tracker_status::test_tracker_status_normalizes_identity_and_url` | pass |
| `TestTrackerStatusForwardsBoundAttributesToDownstreamHandler` | `engine_tracker_status::test_tracker_status_forwards_bound_attributes_to_downstream_handler` | pass |
| `TestStatusUpdatedConnectionErrorUsesErrorWhenIdentityIsPresent` | `engine_tracker_status::test_status_updated_connection_error_uses_error_when_identity_is_present` | pass |
| `TestBuildTrackerViewsMarksAttachedTrackerSupplemental` | `engine_tracker_status::test_build_tracker_views_marks_attached_tracker_supplemental` | pass |
| `TestBuildTrackerViewsAttachedSupplementAppearsOnce` | `engine_tracker_status::test_build_tracker_views_attached_supplement_appears_once` | pass |
| `TestStatusUpdatedCallback` | `engine_tracker_status::test_status_updated_callback` | pass |
| `TestPublicTrackerParsing` | `engine_trackers::test_public_tracker_parsing` | pass |
| `TestTrackerCacheConditionalRefreshAndFailure` | `engine_trackers::test_tracker_cache_conditional_refresh_and_failure` | pass |
| `TestTrackerCacheCorruptionAndFallback` | `engine_trackers::test_tracker_cache_corruption_and_fallback` | pass |
| `TestTrackerFetchIsAsyncAndCancelledOnClose` | `engine_trackers::test_tracker_fetch_is_async_and_cancelled_on_close` | pass |
| `TestTrackerSupplementPublicPrivateAndMagnet` | `engine_trackers::test_tracker_supplement_public_private_and_magnet` | waiting: magnet trackers grouped into original tier |
| `TestTrackerRefreshIsBoundedForExistingTorrents` | `engine_trackers::test_tracker_refresh_is_bounded_for_existing_torrents` | pass |
| `TestTrackerDiscoveredPeerSurvivesRefresh` | `engine_trackers::test_tracker_discovered_peer_survives_refresh` | waiting: supplemental tracker discovery retains live connections |
| `TestManagedTrackersDoNotAccumulateAcrossRestore` | `engine_trackers::test_managed_trackers_do_not_accumulate_across_restore` | pass |
| `TestAddMagnetIgnoresDHTLabelAndKeepsTrackerURLs` | `engine_trackers::test_add_magnet_ignores_dht_label_and_keeps_tracker_ur_ls` | pass |
| `TestSuppliedTrackerValidationPreservesPrivatePasskeysAndTiers` | `engine_trackers::test_supplied_tracker_validation_preserves_private_passkeys_and_tiers` | pass |

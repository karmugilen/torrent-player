# wt-storage Go → Rust test parity

Integration tests live in `tests/port_*.rs` with helpers in `tests/support/`.
Fixtures use `/home/kar/.cache/wt-tmp/wt-storage-tests` via `tempfile` (deleted on drop).

Status legend: **pass** · **ignored-waiting** · **fails**

## Mapping

| Go test | Rust test | Status |
|---|---|---|
| `TestPieceLRUOrdersByUseAndRemoves` (`cache_lru_test.go`) | `port_cache_lru::test_piece_lru_orders_by_use_and_removes` | pass |
| `TestPieceLRUTouchDoesNotGrowWithTheCache` | `port_cache_lru::test_piece_lru_touch_does_not_grow_with_the_cache` | pass |
| `TestMemoryLimitCoversEveryOpenCache` | `port_cache_lru::test_memory_limit_covers_every_open_cache` | pass |
| `TestCacheStatusAndClearCategories` (`cache_store_test.go`) | `port_cache_store::test_cache_status_and_clear_categories` | pass (store/clear helpers; no HTTP `/cache`) |
| `TestPeerHintsPersistPublicOnlyAndExpire` | `port_cache_store::test_peer_hints_persist_public_only_and_expire` | pass |
| `TestDHTContactsRoundTripBounded` | `port_cache_store::test_dht_contacts_round_trip_bounded` | pass |
| `TestResumeWindowPersistsVerifiedBytesOnly` | `port_cache_store::test_resume_window_persists_verified_bytes_only` | pass |
| `TestForgetPeersForADroppedTorrent` | `port_cache_store::test_forget_peers_for_a_dropped_torrent` | pass (store.forget; no `/remove`) |
| `TestForgetPeerHintsOnRemovePath` | `port_cache_store::test_forget_peer_hints_on_remove_path` | pass |
| `TestDHTContactKeepsRealPort` | `port_cache_store::test_dht_contact_keeps_real_port` | pass |
| `TestPeerHintRejectsLoopbackAndRequiresListenPort` | `port_cache_store::test_peer_hint_rejects_loopback_and_requires_listen_port` | pass (loopback only; dial-endpoint is engine) |
| `TestResumeWindowRejectsChecksumMismatch` | `port_cache_store::test_resume_window_rejects_checksum_mismatch` | pass |
| `TestCacheClearDHTRemovesMemoryAndFile` | `port_cache_store::test_cache_clear_dht_removes_memory_and_file` | pass |
| `TestCacheClearRejectsUnknownCategory` | `port_cache_store::test_cache_clear_rejects_unknown_category` | pass |
| `TestDropKeepsPeerHintsAndDeleteForgetsThem` | `port_cache_store::test_drop_keeps_peer_hints_and_delete_forgets_them` | pass (store-layer stand-in for `/remove`) |
| `TestPieceCheckpointRoundTrip` (`checkpoint_test.go`) | `port_checkpoint::test_piece_checkpoint_round_trip` | pass |
| `TestCheckpointSkipsTrustedPiecesOnResume` | `port_checkpoint::test_checkpoint_skips_trusted_pieces_on_resume` | pass |
| `TestCheckpointSurvivesMtimeBumpOnSameInode` | `port_checkpoint::test_checkpoint_survives_mtime_bump_on_same_inode` | pass |
| `TestCheckpointIgnoredWhenFileIdentityChanges` | `port_checkpoint::test_checkpoint_ignored_when_file_identity_changes` | pass |
| `TestCheckpointDeletedOnRemove` | `port_checkpoint::test_checkpoint_deleted_on_remove` | pass (`delete_piece_checkpoint`) |
| `TestDrainSyncFailurePreservesDirtyAndPending` (`checkpoint_rel_test.go`) | `port_checkpoint::test_drain_sync_failure_preserves_dirty_and_pending` | pass |
| `TestDrainCheckpointWriteFailurePreservesDirty` | `port_checkpoint::test_drain_checkpoint_write_failure_preserves_dirty` | pass |
| `TestDrainCheckpointSucceedsAfterTransientFailure` | `port_checkpoint::test_drain_checkpoint_succeeds_after_transient_failure` | pass |
| `TestDrainDoesNotRewriteDeletedCheckpoint` | `port_checkpoint::test_drain_does_not_rewrite_deleted_checkpoint` | pass |
| `TestMalformedCheckpointPieceCountRejected` | `port_checkpoint::test_malformed_checkpoint_piece_count_rejected` | pass |
| `TestCheckpointDeleteDuringPublishDoesNotResurrect` | `port_checkpoint::test_checkpoint_delete_during_publish_does_not_resurrect` | pass |
| `TestCheckpointPersistErrorSurfacedInTorrentStatus` | `port_checkpoint::test_checkpoint_persist_error_surfaced_in_torrent_status` | pass (`checkpoint_persist_error` API smoke) |
| `TestHashExtentsMatchesSHA1AndPread` (`extent_test.go`) | `port_extent::test_hash_extents_matches_sha1_and_pread` | pass |
| `TestHashExtentsAllHoleMatchesZeroHash` | `port_extent::test_hash_extents_all_hole_matches_zero_hash` | pass |
| `TestRangeCompleteUsesDurableBits` | `port_extent::test_range_complete_uses_durable_bits` | pass |
| `TestCompressPieceRunsCountsOnlyWantedPartialsWhilePlaying` (`pieces_test.go`) | `port_pieces::test_compress_piece_runs_counts_only_wanted_partials_while_playing` | pass |
| `TestCompressPieceRuns` | `port_pieces::test_compress_piece_runs` | pass |
| `TestCompressPieceRunsRejectsBadLengths` | `port_pieces::test_compress_piece_runs_rejects_bad_lengths` | pass |
| `TestCompressPieceRunsUint32Limit` | `port_pieces::test_compress_piece_runs_uint32_limit` | pass |
| `TestAggregatePieceBuckets` | `port_pieces::test_aggregate_piece_buckets` | pass |
| `TestAggregatePieceBucketsRejectsInvalidInput` (`pieces_host_test.go`) | `port_pieces::test_aggregate_piece_buckets_rejects_invalid_input` | pass |
| `TestAggregatePieceBucketsSweepMatchesCompressedRuns` | `port_pieces::test_aggregate_piece_buckets_sweep_matches_compressed_runs` | pass |
| `overlapping_files_and_complete_precedence` (`engine-rust/src/lib.rs`) | `port_pieces::overlapping_files_and_complete_precedence` | pass |
| `invalid_input_does_not_write_output` | `port_pieces::invalid_input_does_not_write_output` | pass |
| `empty_and_maximum_piece_count` | `port_pieces::empty_and_maximum_piece_count` | pass |
| `bucket_geometry_for_selection_window` | `port_pieces::bucket_geometry_for_selection_window` | pass |
| `adjacent_duplicate_and_unsorted_ranges_count_once` | `port_pieces::adjacent_duplicate_and_unsorted_ranges_count_once` | pass |
| `adjacent_equal_flag_runs_match_merged_run` | `port_pieces::adjacent_equal_flag_runs_match_merged_run` | pass |
| `bad_inputs_do_not_write_output` | `port_pieces::bad_inputs_do_not_write_output` | pass |
| `brute_force_oracle_matches_expanded_counts` | `port_pieces::brute_force_oracle_matches_expanded_counts` | pass |
| `TestDirectPlayPieceGridTracksRAMReplacement` (`pieces_stream_test.go`) | `port_pieces_stream::test_direct_play_piece_grid_tracks_ram_replacement` | pass |
| `TestStreamPieceGridPartialReplacementAndClose` | `port_pieces_stream::test_stream_piece_grid_partial_replacement_and_close` | pass |
| `TestReceivedRangesMergeLikeASortedMerge` (`stream_ranges_test.go`) | `port_stream_ranges::test_received_ranges_merge_like_a_sorted_merge` | pass |
| `TestSelectedFileBoundaryRetainsWholePiece` (`storage_test.go`) | `port_storage::test_selected_file_boundary_retains_whole_piece` | pass |
| `TestClosedStorageIsNotReused` | `port_storage::test_closed_storage_is_not_reused` | pass |
| `TestPreparedCacheLimitIsEnforced` | `port_storage::test_prepared_cache_limit_is_enforced` | pass |
| `TestAttachDescriptorsRetainsUnselectedExternalFiles` | `port_storage::test_attach_descriptors_retains_unselected_external_files` | pass |
| `TestWriteBackCacheServesFromRAMBeforeDisk` | `port_storage::test_write_back_cache_serves_from_ram_before_disk` | pass |
| `TestFlushWritesOnlyBytesReceivedSinceLastFlush` | `port_storage::test_flush_writes_only_bytes_received_since_last_flush` | pass |
| `TestWriteSlotLoadsSavedBytesOnlyWhenRead` | `port_storage::test_write_slot_loads_saved_bytes_only_when_read` | pass |
| `TestFullyWrittenSlotIsNotPartial` | `port_storage::test_fully_written_slot_is_not_partial` | pass |
| `TestReadMissReadsOnlyRequestedBytes` | `port_storage::test_read_miss_reads_only_requested_bytes` | pass |
| `TestAttachWritesOnlyPreparedBytes` | `port_storage::test_attach_writes_only_prepared_bytes` | pass |
| `TestCloseReportsFailedWriteBack` | `port_storage::test_close_reports_failed_write_back` | pass |
| `TestEmptyPreparedWriteKeepsExistingFile` | `port_storage::test_empty_prepared_write_keeps_existing_file` | pass |
| `TestFailedWriteBackIsReportedAndBacksOff` | `port_storage::test_failed_write_back_is_reported_and_backs_off` | pass |
| `TestEvictedPartialPieceLoadsOnNextWrite` | `port_storage::test_evicted_partial_piece_loads_on_next_write` | pass |
| `TestMarkCompletePersistsBytesAndClearsDirty` | `port_storage::test_mark_complete_persists_bytes_and_clears_dirty` | pass |
| `TestWriteBackCacheEvictsLRUAfterFlush` | `port_storage::test_write_back_cache_evicts_lru_after_flush` | pass |
| `TestWriteBackPartialWriteAfterEvictReloadsDisk` | `port_storage::test_write_back_partial_write_after_evict_reloads_disk` | pass |
| `TestDeleteEngineOwnedStorageWipesStreamAndBoundary` | `port_storage::test_delete_engine_owned_storage_wipes_stream_and_boundary` | pass |
| `TestFilesOverlappingFindsOnlyFilesInRange` | `port_storage::test_files_overlapping_finds_only_files_in_range` | pass |
| `TestDirectPlayPersistsOnlyVerifiedStartupAndRejectsCorruption` (`direct_storage_test.go`) | `port_direct_storage::test_direct_play_persists_only_verified_startup_and_rejects_corruption` | pass |
| `TestDirectPlayEvictsPartialWithoutReturningFabricatedBytes` | `port_direct_storage::test_direct_play_evicts_partial_without_returning_fabricated_bytes` | pass |
| `TestDirectPlayProtectsFullyReceivedPiecesUntilVerification` | `port_direct_storage::test_direct_play_protects_fully_received_pieces_until_verification` | pass |
| `TestDirectPlayCloseDoesNotJoinBlockedCompletionCallback` | `port_direct_storage::test_direct_play_close_does_not_join_blocked_completion_callback` | pass |
| `TestStartupPruningKeepsActiveReadersCache` | `port_direct_storage::test_startup_pruning_keeps_active_readers_cache` | pass |
| `TestEpisodeSwitchMovesStartupBudgetToOpenedEpisode` | `port_direct_storage::test_episode_switch_moves_startup_budget_to_opened_episode` | pass |
| `TestNextEpisodePreloadHasItsOwnStartupBudget` | `port_direct_storage::test_next_episode_preload_has_its_own_startup_budget` | pass |
| `TestZeroPieceHashMatchesSHA1` (`restore_hash_test.go`) | `port_restore_hash::test_zero_piece_hash_matches_sha1` | pass |
| `TestSparsePieceExtentAllHoles` | `port_restore_hash::test_sparse_piece_extent_all_holes` | pass |
| `TestRestoreVerifySelfHashUsesZeroHashForHoles` | `port_restore_hash::test_restore_verify_self_hash_uses_zero_hash_for_holes` | pass |
| `TestRestoreVerifyPieceTypeOnlyWhileEnabled` | `port_restore_hash::test_restore_verify_piece_type_only_while_enabled` | pass |
| `TestSavedDataVerificationRunsIndependentPiecesConcurrently` (`verification_test.go`) | `port_verification::test_saved_data_verification_runs_independent_pieces_concurrently` | pass |
| `TestRestoreOutlivesRequestAndPauseResumeRemainResponsive` | `port_verification::test_restore_outlives_request_and_pause_resume_remain_responsive` | pass |
| `TestFailedRestoreCanRetryOnResume` | `port_verification::test_failed_restore_can_retry_on_resume` | pass |
| `TestRemoveAndShutdownCancelSavedDataCheck` | `port_verification::test_remove_and_shutdown_cancel_saved_data_check` | pass |
| `TestRestartPreservesValidPiecesAndDownloadsOnlyMissingData` | `port_verification::test_restart_preserves_valid_pieces_and_downloads_only_missing_data` | pass |

## Counts (verified 2026-10-27, accessors dispatch)

`CARGO_TARGET_DIR=… CARGO_BUILD_JOBS=3 cargo test -p wt-storage -- --test-threads=1`

| Suite | pass | ignored | fail |
|---|---:|---:|---:|
| Combined (`cargo test -p wt-storage`) | **109** | **0** | **0** |

- Mapped Go (+ engine-rust oracle) tests in this document: **83**.
- Every previously ignored test is now **pass** (accessors + fault hooks landed in `storage.rs`).
- `cargo fmt -p wt-storage` clean; `cargo clippy -p wt-storage --all-targets -- -D warnings` clean.

## New public methods (forward to agy-4)

| Method | Role |
|---|---|
| `flush_all_dirty(&self)` | Go `flushAllDirtyLocked` under the lock; sets `write_back_error` / `flush_retry` on failure |
| `files_overlapping(&[FileDescriptor], start, end)` | Go `filesOverlapping` binary-search slice |
| `stream_piece_runs(&self, total)` | Go `streamPieceRuns` RAM grid snapshot |
| `set_on_stream_evict(Option<Arc<dyn Fn(usize)+Send+Sync>>)` | Go `onStreamEvict`; fired async on Direct Play `mark_complete`; `close` does not join |
| `cache_bytes()` / `max_cache_bytes()` | Go observers |
| `clear_startup_pieces(&self)` | Clear Direct Play startup protection (tiny-piece fixtures) |
| `replace_file_handle(index, File)` | Test seam for read-only write-back failures |
| `set_file_sync_hook` / `set_write_checkpoint_hook` | Drain fault injection (`FileSyncHook` / `WriteCheckpointHook`) |
| `drain_sync_and_checkpoint(&self)` | Go `drainSyncAndCheckpoint` (unlocks around hooks) |
| `flush_retry()` / `checkpoint_gen()` / `set_checkpoint_dirty` / `note_pending_sync_fd` | Test helpers |
| Direct Play `close` | Skips flush when `stream` (files never configured) |

## Remaining behavioural notes (not blockers)

1. **Cache limit after `attach_direct_play`** — floors to `DEFAULT_STREAM_CACHE_MAX_BYTES` then raises to `DIRECT_PLAY_MIN_PIECES * piece_len`; tests that need a tighter budget call `set_cache_limit` again after attach (Go parity).
2. **Write-back backoff** — sync `flush_all_dirty` updates `flush_retry`; no async `scheduleDiskFlushLocked` goroutine yet (coordinator chose sync hooks).
3. Engine HTTP control paths (`/cache`, `/remove`, `/pieces`, `/configure`) stay out of crate scope.

## Notes

- `PieceStateRun` has no `Ok` / `QueuedForHash` / `Marking` fields; `compress_piece_runs` ignores them as in Go’s class collapse.
- `set_preload_file(usize::MAX)` stands in for Go’s `setPreloadFile(-1)` cancel.
- Sparse-hole assertions skip cleanly when the filesystem does not expose `SEEK_DATA`/`SEEK_HOLE`.
- Fixtures: `tests/support/` → tempfile under `/home/kar/.cache/wt-tmp/wt-storage-tests` (≤256 MiB, deleted on drop).

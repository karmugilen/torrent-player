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
| `TestDrainSyncFailurePreservesDirtyAndPending` (`checkpoint_rel_test.go`) | `port_checkpoint::test_drain_sync_failure_preserves_dirty_and_pending` | ignored-waiting |
| `TestDrainCheckpointWriteFailurePreservesDirty` | `port_checkpoint::test_drain_checkpoint_write_failure_preserves_dirty` | ignored-waiting |
| `TestDrainCheckpointSucceedsAfterTransientFailure` | `port_checkpoint::test_drain_checkpoint_succeeds_after_transient_failure` | ignored-waiting |
| `TestDrainDoesNotRewriteDeletedCheckpoint` | `port_checkpoint::test_drain_does_not_rewrite_deleted_checkpoint` | ignored-waiting |
| `TestMalformedCheckpointPieceCountRejected` | `port_checkpoint::test_malformed_checkpoint_piece_count_rejected` | pass |
| `TestCheckpointDeleteDuringPublishDoesNotResurrect` | `port_checkpoint::test_checkpoint_delete_during_publish_does_not_resurrect` | ignored-waiting |
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
| `TestDirectPlayPieceGridTracksRAMReplacement` (`pieces_stream_test.go`) | `port_pieces_stream::test_direct_play_piece_grid_tracks_ram_replacement` | ignored-waiting |
| `TestStreamPieceGridPartialReplacementAndClose` | `port_pieces_stream::test_stream_piece_grid_partial_replacement_and_close` | ignored-waiting |
| `TestReceivedRangesMergeLikeASortedMerge` (`stream_ranges_test.go`) | `port_stream_ranges::test_received_ranges_merge_like_a_sorted_merge` | pass |
| `TestSelectedFileBoundaryRetainsWholePiece` (`storage_test.go`) | `port_storage::test_selected_file_boundary_retains_whole_piece` | pass |
| `TestClosedStorageIsNotReused` | `port_storage::test_closed_storage_is_not_reused` | pass |
| `TestPreparedCacheLimitIsEnforced` | `port_storage::test_prepared_cache_limit_is_enforced` | pass |
| `TestAttachDescriptorsRetainsUnselectedExternalFiles` | `port_storage::test_attach_descriptors_retains_unselected_external_files` | pass |
| `TestWriteBackCacheServesFromRAMBeforeDisk` | `port_storage::test_write_back_cache_serves_from_ram_before_disk` | pass |
| `TestFlushWritesOnlyBytesReceivedSinceLastFlush` | `port_storage::test_flush_writes_only_bytes_received_since_last_flush` | ignored-waiting |
| `TestWriteSlotLoadsSavedBytesOnlyWhenRead` | `port_storage::test_write_slot_loads_saved_bytes_only_when_read` | pass |
| `TestFullyWrittenSlotIsNotPartial` | `port_storage::test_fully_written_slot_is_not_partial` | pass |
| `TestReadMissReadsOnlyRequestedBytes` | `port_storage::test_read_miss_reads_only_requested_bytes` | pass |
| `TestAttachWritesOnlyPreparedBytes` | `port_storage::test_attach_writes_only_prepared_bytes` | pass |
| `TestCloseReportsFailedWriteBack` | `port_storage::test_close_reports_failed_write_back` | ignored-waiting |
| `TestEmptyPreparedWriteKeepsExistingFile` | `port_storage::test_empty_prepared_write_keeps_existing_file` | pass |
| `TestFailedWriteBackIsReportedAndBacksOff` | `port_storage::test_failed_write_back_is_reported_and_backs_off` | ignored-waiting |
| `TestEvictedPartialPieceLoadsOnNextWrite` | `port_storage::test_evicted_partial_piece_loads_on_next_write` | pass |
| `TestMarkCompletePersistsBytesAndClearsDirty` | `port_storage::test_mark_complete_persists_bytes_and_clears_dirty` | pass |
| `TestWriteBackCacheEvictsLRUAfterFlush` | `port_storage::test_write_back_cache_evicts_lru_after_flush` | pass |
| `TestWriteBackPartialWriteAfterEvictReloadsDisk` | `port_storage::test_write_back_partial_write_after_evict_reloads_disk` | ignored-waiting |
| `TestDeleteEngineOwnedStorageWipesStreamAndBoundary` | `port_storage::test_delete_engine_owned_storage_wipes_stream_and_boundary` | pass |
| `TestFilesOverlappingFindsOnlyFilesInRange` | `port_storage::test_files_overlapping_finds_only_files_in_range` | ignored-waiting |
| `TestDirectPlayPersistsOnlyVerifiedStartupAndRejectsCorruption` (`direct_storage_test.go`) | `port_direct_storage::test_direct_play_persists_only_verified_startup_and_rejects_corruption` | pass |
| `TestDirectPlayEvictsPartialWithoutReturningFabricatedBytes` | `port_direct_storage::test_direct_play_evicts_partial_without_returning_fabricated_bytes` | pass |
| `TestDirectPlayProtectsFullyReceivedPiecesUntilVerification` | `port_direct_storage::test_direct_play_protects_fully_received_pieces_until_verification` | pass |
| `TestDirectPlayCloseDoesNotJoinBlockedCompletionCallback` | `port_direct_storage::test_direct_play_close_does_not_join_blocked_completion_callback` | ignored-waiting |
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

## Counts (verified 2026-10-27)

`CARGO_TARGET_DIR=…/wt-storage CARGO_BUILD_JOBS=3 cargo test -p wt-storage --tests -- --test-threads=1`

| Suite | pass | ignored | fail |
|---|---:|---:|---:|
| Integration `tests/port_*.rs` | **70** | **13** | **0** |
| Lib unit tests (agy-3 / grok-5) | 23 | 3 | 0 |
| Combined (`--tests`) | 93 | 16 | 0 |

- Mapped Go (+ engine-rust oracle) tests in this document: **83** (every Go storage test listed in the task + `engine-rust/src/lib.rs` oracles including `brute_force_oracle_matches_expanded_counts`).
- All non-ignored integration tests **pass**. Zero fails.
- `cargo fmt -p wt-storage` applied to owned test sources.
- `cargo clippy -p wt-storage --tests -- -D warnings` **blocked by lib** (outside ownership): unused `PieceStorage` imports in `src/verification.rs` / `src/direct_storage.rs`, `clippy::suspicious_open_options` in `direct_storage.rs`, `clippy::needless_borrows_for_generic_args` in `zero_hash.rs`. Forward to agy-3 / grok-5.

## API gaps (forward to agy-3)

1. **`DocumentTorrentStorage::flush_all_dirty(&self) -> StorageResult<()>`** — today only `flush_all_dirty_locked(&mut StorageInner)`; integration tests cannot flush between writes without `close`/`mark_complete`. Blocks: `TestFlushWritesOnlyBytesReceivedSinceLastFlush`, `TestWriteBackPartialWriteAfterEvictReloadsDisk`.

2. **`files_overlapping(files, start, end) -> Vec`/`&[FileDescriptor]`** — Go package helper; not exported. Blocks: `TestFilesOverlappingFindsOnlyFilesInRange`.

3. **`DocumentTorrentStorage::stream_piece_runs(total) -> StorageResult<Vec<PieceRun>>`** — Direct Play / stream RAM grid snapshot. Blocks: both `pieces_stream_test.go` cases.

4. **Public `set_on_stream_evict` / `on_stream_evict` setter** (or test hook) — Go assigns `st.onStreamEvict` for completion-callback / close deadlock test. Blocks: `TestDirectPlayCloseDoesNotJoinBlockedCompletionCallback`. (Also listed by grok-5.)

5. **Fault injection for drain/fsync/checkpoint write failures** — Go `checkpoint_rel_test.go` forces sync/write errors and drain-vs-delete races. Blocks: five drain/race cases.

6. **Injectable write-back failure** (replace backing fd with read-only / force write error + backoff). Blocks: `TestCloseReportsFailedWriteBack`, `TestFailedWriteBackIsReportedAndBacksOff`.

7. **Direct Play accessors (grok-5 notes):** `cache_bytes()`, `max_cache_bytes()`, `clear_startup_pieces()` — useful for Direct Play / startup-budget assertions without poking internals.

8. **Direct Play `close()` flush behaviour** — after `attach_direct_play`, stream-mode flush fails with `"storage for … is not configured"` when file descriptors are not attached for write-back. Tests use a `close_direct` helper that tolerates that error. Go skips flush in pure stream mode; Rust should match (skip flush or treat as success when no durable files are configured).

9. **Cache limit after `attach_direct_play`** — `attach_direct_play` floors the cache to `DEFAULT_STREAM_CACHE_MAX_BYTES`; tests that need a tighter budget must call `set_cache_limit` again after attach. Documented behaviour; confirm intentional.

## Notes

- Engine HTTP control paths (`/cache`, `/remove`, `/pieces`, `/configure`) are out of crate scope; equivalents use `wt-storage` public helpers.
- `PieceStateRun` has no `Ok` / `QueuedForHash` / `Marking` fields; `compress_piece_runs` ignores them as in Go’s class collapse.
- `set_preload_file(usize::MAX)` stands in for Go’s `setPreloadFile(-1)` cancel.
- Sparse-hole assertions skip cleanly when the filesystem does not expose `SEEK_DATA`/`SEEK_HOLE`.
- Fixtures: `tests/support/` → tempfile under `/home/kar/.cache/wt-tmp/wt-storage-tests` (≤256 MiB, deleted on drop).
- Owned paths only: `tests/port_*.rs`, `tests/support/`, `tests/STORAGE_TESTS.md`; `[dev-dependencies]` added (tempfile, rand, sha1, hex, serde_json). `src/` untouched.

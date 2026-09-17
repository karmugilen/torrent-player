package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TransferDecisionTest {
    private fun input(
        intent: TransferIntent = TransferIntent.RUN,
        engineReady: Boolean = true,
        metadataReady: Boolean = true,
        hasSelection: Boolean = true,
        storage: StorageReadiness = StorageReadiness.READY,
        networkAllowed: Boolean = true,
        verificationActive: Boolean = false,
    ) = TransferDecisionInput(
        intent, engineReady, metadataReady, hasSelection, storage, networkAllowed, verificationActive,
    )

    @Test fun manual_pause_and_delete_stop_every_kind_of_network_work() {
        listOf(TransferIntent.PAUSED, TransferIntent.DELETED).forEach { intent ->
            val decision = decideTransfer(input(intent = intent, metadataReady = false))
            assertFalse(decision.discoveryAllowed)
            assertFalse(decision.payloadAllowed)
        }
        assertEquals(TransferStage.PAUSED, decideTransfer(input(intent = TransferIntent.PAUSED)).stage)
        assertEquals(TransferStage.DELETED, decideTransfer(input(intent = TransferIntent.DELETED)).stage)
    }

    @Test fun pending_magnet_keeps_discovery_without_payload_or_selection() {
        val decision = decideTransfer(input(metadataReady = false, hasSelection = false))
        assertEquals(TransferStage.FINDING_METADATA, decision.stage)
        assertTrue(decision.discoveryAllowed)
        assertFalse(decision.payloadAllowed)
    }

    @Test fun engine_and_network_are_prerequisites_for_discovery() {
        assertEquals(TransferStage.STARTING_ENGINE, decideTransfer(input(engineReady = false, metadataReady = false)).stage)
        assertEquals(TransferStage.WAITING_FOR_NETWORK, decideTransfer(input(networkAllowed = false, metadataReady = false)).stage)
        assertFalse(decideTransfer(input(networkAllowed = false, metadataReady = false)).discoveryAllowed)
    }

    @Test fun verification_allows_discovery_but_never_payload() {
        val decision = decideTransfer(input(verificationActive = true))
        assertEquals(TransferStage.VERIFYING_SAVED_DATA, decision.stage)
        assertTrue(decision.discoveryAllowed)
        assertFalse(decision.payloadAllowed)
    }

    @Test fun explicit_empty_selection_stops_payload_after_metadata_is_known() {
        val decision = decideTransfer(input(hasSelection = false))
        assertEquals(TransferStage.WAITING_FOR_SELECTION, decision.stage)
        assertFalse(decision.discoveryAllowed)
        assertFalse(decision.payloadAllowed)
    }

    @Test fun storage_failures_are_actionable_and_do_not_request_payload() {
        assertEquals(TransferStage.STORAGE_ACCESS_REQUIRED, decideTransfer(input(storage = StorageReadiness.ACCESS_REQUIRED)).stage)
        assertEquals(TransferStage.STORAGE_ERROR, decideTransfer(input(storage = StorageReadiness.FATAL_ERROR)).stage)
        val transient = decideTransfer(input(storage = StorageReadiness.TRANSIENT_ERROR))
        assertEquals(TransferStage.RETRYING, transient.stage)
        assertFalse(transient.payloadAllowed)
    }

    @Test fun healthy_run_allows_discovery_and_payload_without_slot_limits() {
        val active = decideTransfer(input())
        assertEquals(TransferStage.DOWNLOADING, active.stage)
        assertTrue(active.discoveryAllowed)
        assertTrue(active.payloadAllowed)
    }

    @Test fun retry_has_three_bounded_backoff_attempts() {
        val episode = RetryEpisode(id = 8, generation = 3)
        val first = RetryPolicy.onTransientFailure(null, episode, TransferIntent.RUN).schedule()
        assertEquals(1, first.state.attempt)
        assertEquals(5_000L, first.afterMillis)

        val second = RetryPolicy.onTransientFailure(
            RetryPolicy.markAttemptStarted(first.state, episode)!!, episode, TransferIntent.RUN,
        ).schedule()
        assertEquals(2, second.state.attempt)
        assertEquals(15_000L, second.afterMillis)

        val third = RetryPolicy.onTransientFailure(
            RetryPolicy.markAttemptStarted(second.state, episode)!!, episode, TransferIntent.RUN,
        ).schedule()
        assertEquals(3, third.state.attempt)
        assertEquals(45_000L, third.afterMillis)

        assertEquals(
            RetryDirective.Exhausted,
            RetryPolicy.onTransientFailure(
                RetryPolicy.markAttemptStarted(third.state, episode)!!, episode, TransferIntent.RUN,
            ),
        )
    }

    @Test fun duplicate_failure_never_adds_a_second_timer() {
        val episode = RetryEpisode(1, 4)
        val scheduled = RetryPolicy.onTransientFailure(null, episode, TransferIntent.RUN).schedule()
        assertEquals(
            RetryDirective.AlreadyScheduled,
            RetryPolicy.onTransientFailure(scheduled.state, episode, TransferIntent.RUN),
        )
    }

    @Test fun stale_generation_cannot_touch_current_retry() {
        val currentEpisode = RetryEpisode(1, 6)
        val staleEpisode = RetryEpisode(1, 5)
        val current = RetryPolicy.onTransientFailure(null, currentEpisode, TransferIntent.RUN).schedule().state
        assertEquals(RetryDirective.IgnoreStaleEvent, RetryPolicy.onTransientFailure(current, staleEpisode, TransferIntent.RUN))
        assertNull(RetryPolicy.markAttemptStarted(current, staleEpisode))
    }

    @Test fun pause_or_delete_cancels_pending_retry() {
        val episode = RetryEpisode(7, 2)
        val state = RetryPolicy.onTransientFailure(null, episode, TransferIntent.RUN).schedule().state
        assertEquals(RetryDirective.Cancel, RetryPolicy.onIntentChanged(state, TransferIntent.PAUSED))
        assertEquals(RetryDirective.Cancel, RetryPolicy.onTransientFailure(state, episode, TransferIntent.DELETED))
    }

    @Test fun network_recovery_pulls_one_pending_retry_forward_once() {
        val episode = RetryEpisode(9, 1)
        val state = RetryPolicy.onTransientFailure(null, episode, TransferIntent.RUN).schedule().state
        val immediate = RetryPolicy.onNetworkRecovered(state, episode, TransferIntent.RUN).schedule()
        assertEquals(0L, immediate.afterMillis)
        assertTrue(immediate.state.networkRecoveryUsed)
        assertEquals(RetryDirective.NoChange, RetryPolicy.onNetworkRecovered(immediate.state, episode, TransferIntent.RUN))
    }

    @Test fun recovery_for_no_or_stale_retry_does_not_start_an_unbounded_cycle() {
        val episode = RetryEpisode(2, 1)
        assertEquals(RetryDirective.IgnoreStaleEvent, RetryPolicy.onNetworkRecovered(null, episode, TransferIntent.RUN))
        assertEquals(RetryDirective.NoChange, RetryPolicy.onNetworkRecovered(null, episode, TransferIntent.PAUSED))
    }

    private fun RetryDirective.schedule(): RetryDirective.Schedule {
        assertTrue(this is RetryDirective.Schedule)
        return this as RetryDirective.Schedule
    }
}

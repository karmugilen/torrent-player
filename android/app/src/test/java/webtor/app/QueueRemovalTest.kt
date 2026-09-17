package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class QueueRemovalTest {
    @Test
    fun legacy_queue_pref_keys_are_explicit_and_cleared_on_upgrade() {
        assertEquals(
            listOf("downloadQueueMode", "downloadQueueOrder"),
            LEGACY_DOWNLOAD_QUEUE_PREF_KEYS,
        )
    }

    @Test
    fun upgrade_restores_saved_selection_without_starting_user_paused() {
        assertTrue(
            shouldRestorePayloadSelection(
                paused = false,
                isDeleting = false,
                complete = false,
                metadataReady = true,
                hasSelection = true,
            ),
        )
        assertFalse(
            shouldRestorePayloadSelection(
                paused = true,
                isDeleting = false,
                complete = false,
                metadataReady = true,
                hasSelection = true,
            ),
        )
        assertFalse(
            shouldRestorePayloadSelection(
                paused = false,
                isDeleting = false,
                complete = false,
                metadataReady = true,
                hasSelection = false,
            ),
        )
        assertFalse(
            shouldRestorePayloadSelection(
                paused = false,
                isDeleting = false,
                complete = true,
                metadataReady = true,
                hasSelection = true,
            ),
        )
    }

    @Test
    fun transfer_policy_has_no_download_slot_gate() {
        val decision = decideTransfer(
            TransferDecisionInput(
                intent = TransferIntent.RUN,
                engineReady = true,
                metadataReady = true,
                hasSelection = true,
                storage = StorageReadiness.READY,
                networkAllowed = true,
                verificationActive = false,
            ),
        )
        assertEquals(TransferStage.DOWNLOADING, decision.stage)
        assertTrue(decision.payloadAllowed)
        assertTrue(decision.discoveryAllowed)
    }
}

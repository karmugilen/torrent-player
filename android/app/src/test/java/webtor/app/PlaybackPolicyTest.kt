package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PlaybackPolicyTest {
    @Test
    fun playIsBlockedBeforeASessionStarts() {
        assertEquals(LOW_BATTERY_PLAY_MESSAGE, playbackDataBlock(batteryPaused = true, networkAllowed = true, networkReason = null))
        assertEquals("Wi-Fi only", playbackDataBlock(batteryPaused = false, networkAllowed = false, networkReason = "Wi-Fi only"))
        assertEquals(
            "Waiting for an allowed network.",
            playbackDataBlock(batteryPaused = false, networkAllowed = false, networkReason = null),
        )
        assertNull(playbackDataBlock(batteryPaused = false, networkAllowed = true, networkReason = null))
    }
}

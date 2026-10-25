package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The built-in playback mode: these are fixed, no longer user switches. */
class SettingsDefaultsTest {
    @Test
    fun recommendedSettingsStartOn() {
        val state = UiState()
        assertTrue(state.ghostProMode)
        assertTrue(state.adaptiveBuffer)
        assertTrue(state.playBoost)
        assertTrue(state.playNext)
        assertTrue(state.fileOnlyPlayback)
        assertTrue(state.pauseOnLowBattery)
        assertTrue(state.claudeSeekMode)
        assertEquals(15, state.idleDropMinutes)
    }

    @Test
    fun settingsCoveredByGhostProStartOff() {
        val state = UiState()
        assertFalse(state.ghostMode)
        assertFalse(state.peerExploration)
        assertFalse(state.fastPeerSearch)
        assertFalse(state.burstAwareBuffer)
        assertFalse(state.antiDpiEnabled)
    }

    @Test
    fun playOpensWithoutWaitingForPreload() {
        assertFalse(UiState().preloadStart)
    }
}

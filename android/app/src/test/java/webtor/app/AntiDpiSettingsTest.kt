package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.BypassStatus

class AntiDpiSettingsTest {
    @Test
    fun defaultUiStateHasAntiDpiDisabledAndNoFakeBypass() {
        val state = UiState()
        assertFalse(state.antiDpiEnabled)
        assertNull(state.bypassStatus)
    }

    @Test
    fun bypassStatusRepresentsRealActivationAndCoverage() {
        val active = BypassStatus(
            enabled = true,
            active = true,
            mode = "stream-adapt",
            coverage = listOf("outgoing-tcp-peers", "http-trackers"),
            error = null,
        )
        val state = UiState(antiDpiEnabled = true, bypassStatus = active)
        assertTrue(state.antiDpiEnabled)
        assertTrue(state.bypassStatus?.active == true)
        assertEquals("stream-adapt", state.bypassStatus?.mode)
        assertEquals(2, state.bypassStatus?.coverage?.size)
        assertNull(state.bypassStatus?.error)
    }

    @Test
    fun bypassStatusReportsBackendFailureHonestly() {
        val failed = BypassStatus(
            enabled = true,
            active = false,
            mode = "off",
            coverage = emptyList(),
            error = "native bypass backend failed to initialize",
        )
        val state = UiState(antiDpiEnabled = true, bypassStatus = failed)
        assertTrue(state.antiDpiEnabled)
        assertFalse(state.bypassStatus?.active == true)
        assertEquals("native bypass backend failed to initialize", state.bypassStatus?.error)
    }
}

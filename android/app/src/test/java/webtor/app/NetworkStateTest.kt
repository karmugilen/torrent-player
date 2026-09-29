package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NetworkStateTest {
    @Test
    fun lateLostOnOldNetworkDoesNotDropValidatedRoute() {
        val state = DefaultNetworkState()
        state.available(1)
        state.capabilities(1, validated = true, wifi = true, metered = false)
        assertTrue(state.snapshot.connected)
        val wifiGeneration = state.snapshot.generation

        // Cellular becomes default while Wi-Fi loss is still in flight.
        state.available(2)
        state.capabilities(2, validated = true, wifi = false, metered = true)
        assertTrue(state.snapshot.connected)
        assertFalse(state.snapshot.wifi)
        assertTrue(state.snapshot.metered)
        val cellularGeneration = state.snapshot.generation
        assertTrue(cellularGeneration > wifiGeneration)

        // Delayed onLost for the previous network must not clobber the new route.
        val afterStaleLost = state.lost(1)
        assertEquals(cellularGeneration, afterStaleLost.generation)
        assertTrue(afterStaleLost.connected)
        assertFalse(afterStaleLost.wifi)
    }

    @Test
    fun unvalidatedOrBlockedNetworkIsOfflineUntilUsable() {
        val state = DefaultNetworkState()
        state.available(7)
        assertFalse(state.capabilities(7, validated = false, wifi = true, metered = false).connected)
        assertTrue(state.capabilities(7, validated = true, wifi = true, metered = false).connected)
        assertFalse(state.blocked(7, true).connected)
        assertTrue(state.blocked(7, false).connected)
    }

    @Test
    fun linkPropertyChangeBumpsGenerationOnSameNetwork() {
        val state = DefaultNetworkState()
        state.available(3)
        state.capabilities(3, validated = true, wifi = true, metered = false)
        val before = state.snapshot.generation
        val after = state.links(3, "wlan0|addr-a|route-a|dns-a")
        assertTrue(after.connected)
        assertTrue(after.generation > before)
        assertEquals(after.generation, state.links(3, "wlan0|addr-a|route-a|dns-a").generation)
        assertTrue(state.links(3, "wlan0|addr-b|route-a|dns-a").generation > after.generation)
    }
}

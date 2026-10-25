package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Test
import webtor.core.SeekStats

class PlaybackStatsLineTest {
    @Test
    fun emptyUntilThereIsSomethingToShow() {
        assertEquals("", playbackStatsLine(UiState()))
    }

    @Test
    fun showsSeeks() {
        val ui = UiState(seekStats = SeekStats(count = 5, lastMs = 1250, medianMs = 640, stalls = 1))
        assertEquals("Seeks 5 · usually 640 ms · last 1.3 s · 1 stall", playbackStatsLine(ui))
    }
}

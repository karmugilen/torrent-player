package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Test
import webtor.core.SeekStats

class SeekStatsLineTest {
    @Test
    fun promptsUntilASeekIsMeasured() {
        assertEquals("Claude seek off · seek in a video to measure seek time.", seekStatsLine(null, claudeSeek = false))
        assertEquals(
            "Claude seek on · seek in a video to measure seek time.",
            seekStatsLine(SeekStats(0, 0, 0, 0), claudeSeek = true),
        )
    }

    @Test
    fun summarisesMedianLastAndStalls() {
        assertEquals(
            "Claude seek on · 5 seeks · median 0.6 s · last 1.3 s · 1 stall",
            seekStatsLine(SeekStats(count = 5, lastMs = 1250, medianMs = 640, stalls = 1), claudeSeek = true),
        )
        assertEquals(
            "Claude seek off · 2 seeks · median 4.0 s · last 9.1 s · 0 stalls",
            seekStatsLine(SeekStats(count = 2, lastMs = 9100, medianMs = 4000, stalls = 0), claudeSeek = false),
        )
    }
}

package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Test
import webtor.core.SeekStats

class SeekStatsLineTest {
    @Test
    fun promptsUntilASeekIsMeasured() {
        assertEquals("Seek time · seek in a video to measure it.", seekStatsLine(null))
        assertEquals("Seek time · seek in a video to measure it.", seekStatsLine(SeekStats(0, 0, 0, 0)))
    }

    @Test
    fun summarisesMedianLastAndStalls() {
        assertEquals(
            "Seek time · 5 seeks · median 640 ms · last 1250 ms · 1 stall",
            seekStatsLine(SeekStats(count = 5, lastMs = 1250, medianMs = 640, stalls = 1)),
        )
        assertEquals(
            "Seek time · 2 seeks · median 4000 ms · last 9100 ms · 0 stalls",
            seekStatsLine(SeekStats(count = 2, lastMs = 9100, medianMs = 4000, stalls = 0)),
        )
    }
}

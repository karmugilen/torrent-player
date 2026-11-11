package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Test

class PlaybackStatsLineTest {
    @Test
    fun emptyUntilThereIsSomethingToShow() {
        assertEquals("", playbackStatsLine(UiState()))
    }
}

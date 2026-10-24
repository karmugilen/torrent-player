package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.TorrentStatus

class IdleDirectPlayTest {
    private fun status(ready: Boolean = true, readers: Int = 0, resting: Boolean = true) = TorrentStatus(
        "eng", null, null, null, true, false, false, 0.0, 0, 0, 0, 100, 0, 0, emptyList(),
        playbackReaders = readers, resting = resting, idleDropReady = ready,
    )

    private val live = DownloadEntry(
        "k", "Movie", "magnet:x", "meta", "eng", "Direct Play", emptyList(), emptySet(),
        directPlay = true, lifecycleState = EntryLifecycleState.DOWNLOADING,
    )

    @Test
    fun dropsOnlyAnUnattendedLiveDirectPlaySessionTheEngineMarkedReady() {
        assertTrue(shouldDropIdleDirectPlay(live, status(), idleMinutes = 15))
        assertFalse(shouldDropIdleDirectPlay(live, status(), idleMinutes = 0))
        assertFalse(shouldDropIdleDirectPlay(live, status(ready = false), 15))
        assertFalse(shouldDropIdleDirectPlay(live, status(readers = 1), 15))
        assertFalse(shouldDropIdleDirectPlay(live.copy(directPlay = false), status(), 15))
        assertFalse(shouldDropIdleDirectPlay(live.copy(paused = true), status(), 15))
        assertFalse(shouldDropIdleDirectPlay(live.copy(engineId = "other"), status(), 15))
        assertFalse(shouldDropIdleDirectPlay(live.copy(lifecycleState = EntryLifecycleState.STOPPING), status(), 15))
    }

    @Test
    fun labelsShowRestingAndStoppedDirectPlay() {
        val resting = live.copy(status = status(ready = false, readers = 1))
        assertEquals("Direct Play · resting", resting.stateLabel())
        assertEquals("Direct Play", live.copy(status = status(ready = false, resting = false)).stateLabel())
        val stopped = live.copy(engineId = null, paused = true, status = null, lifecycleState = EntryLifecycleState.STOPPED)
        assertEquals("Stopped", stopped.stateLabel())
        assertEquals("Stopped", downloadSummaryLabel(stopped))
    }
}

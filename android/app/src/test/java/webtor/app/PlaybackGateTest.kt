package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.app.PlaybackGate.Command.PAUSE
import webtor.app.PlaybackGate.Command.PLAY

class PlaybackGateTest {
    private fun opened(): PlaybackGate = PlaybackGate().apply {
        engineStarting(); fileStarting(); fileLoaded()
    }

    @Test
    fun aNewFilePausesAndWaitsForItsLength() {
        val gate = PlaybackGate()
        gate.engineStarting()
        assertEquals(listOf(PAUSE), gate.fileStarting())
        assertTrue(gate.view.buffering)
        assertTrue(gate.view.waitingForLength)
    }

    @Test
    fun theLengthStartsPlaybackOnce() {
        val gate = opened()
        assertEquals(listOf(PLAY), gate.lengthKnown(1200.0))
        assertFalse(gate.view.waitingForLength)
        assertEquals(emptyList<PlaybackGate.Command>(), gate.lengthKnown(1200.0))
    }

    @Test
    fun aZeroLengthIsNotALength() {
        val gate = opened()
        assertEquals(emptyList<PlaybackGate.Command>(), gate.lengthKnown(0.0))
        assertTrue(gate.view.waitingForLength)
    }

    @Test
    fun theLengthReadAtOpenCountsToo() {
        val gate = opened()
        assertEquals(listOf(PLAY), gate.lengthAtOpen(95.5))
        assertFalse(gate.view.waitingForLength)
    }

    @Test
    fun playDoesNothingWhileTheLengthMayStillCome() {
        val gate = opened() // opened, length not read yet
        assertEquals(emptyList<PlaybackGate.Command>(), gate.playPressed())
        assertTrue(gate.view.waitingForLength)
    }

    @Test
    fun playEndsTheWaitWhenTheFileHasNoLengthAtAll() {
        val gate = opened()
        gate.lengthAtOpen(null)
        assertEquals(listOf(PLAY), gate.playPressed())
        assertFalse(gate.view.waitingForLength)
    }

    @Test
    fun playIsANormalToggleOnceTheLengthIsKnown() {
        val gate = opened()
        gate.lengthKnown(10.0)
        assertNull(gate.playPressed())
    }

    @Test
    fun theNextFileForgetsThatTheLastOneHadNoLength() {
        val gate = opened()
        gate.lengthAtOpen(null)
        gate.fileStarting()
        gate.fileLoaded()
        assertEquals(emptyList<PlaybackGate.Command>(), gate.playPressed())
    }

    @Test
    fun bufferingIsAnyWait() {
        val gate = opened()
        gate.lengthKnown(10.0)
        gate.playbackRestarted()
        assertFalse(gate.view.buffering)
        gate.cacheWait(true); assertTrue(gate.view.buffering)
        gate.cacheWait(false); assertFalse(gate.view.buffering)
        gate.mpvSeeking(true); assertTrue(gate.view.buffering); assertTrue(gate.view.mpvSeeking)
        gate.playbackRestarted(); assertFalse(gate.view.buffering); assertFalse(gate.view.mpvSeeking)
        gate.switching(); assertTrue(gate.view.buffering)
    }

    @Test
    fun aSeekSpinsUntilFramesComeBackOrItGivesUp() {
        val gate = opened()
        gate.lengthKnown(10.0); gate.playbackRestarted()
        gate.seekRequested()
        assertTrue(gate.view.spinning(paused = true, scrubbing = false))
        assertFalse(gate.view.spinning(paused = true, scrubbing = true))
        gate.playbackRestarted()
        assertFalse(gate.view.seekPending)
        gate.seekRequested(); gate.seekGaveUp()
        assertFalse(gate.view.seekPending)
    }

    @Test
    fun pausedWhileBufferingDoesNotSpinExceptForTheLengthWait() {
        val gate = opened()
        gate.lengthKnown(10.0); gate.playbackRestarted()
        gate.cacheWait(true)
        assertFalse(gate.view.spinning(paused = true, scrubbing = false))
        assertTrue(gate.view.spinning(paused = false, scrubbing = false))
        val waiting = opened()
        assertTrue(waiting.view.spinning(paused = true, scrubbing = false))
    }

    @Test
    fun leavingDropsTheWaitWithoutPlaying() {
        val gate = opened()
        gate.stopped()
        assertFalse(gate.view.waitingForLength)
        assertNull(gate.playPressed())
    }

    @Test
    fun aNewFileClearsAPendingSeek() {
        val gate = opened()
        gate.seekRequested()
        gate.fileStarting()
        assertFalse(gate.view.seekPending)
    }

    @Test
    fun listenersHearOnlyChanges() {
        val seen = mutableListOf<PlaybackGate.View>()
        val gate = PlaybackGate { seen += it }
        gate.cacheWait(false) // nothing changes: still opening
        assertTrue(seen.isEmpty())
        gate.fileLoaded()
        assertEquals(1, seen.size)
        assertFalse(seen.last().buffering)
    }
}

package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import webtor.app.PlayerGestures.Drag
import webtor.app.PlayerGestures.ScrubAction
import webtor.app.PlayerGestures.Swipe

class PlayerGesturesTest {
    private fun drag(
        dx: Float, dy: Float, elapsedMs: Long = 300, startX: Float = 500f,
        holding: Boolean = false, sheetOpen: Boolean = false, compact: Boolean = false,
    ) = PlayerGestures.dragStart(dx, dy, elapsedMs, startX, width = 1000f, holding, sheetOpen, compact)

    @Test
    fun tapSidesAreThirds() {
        assertEquals(-1, PlayerGestures.side(100f, 900f))
        assertEquals(0, PlayerGestures.side(450f, 900f))
        assertEquals(1, PlayerGestures.side(800f, 900f))
    }

    @Test
    fun smallMovesDecideNothing() {
        assertEquals(Drag.NONE, drag(20f, 20f).mode)
    }

    @Test
    fun clearlySidewaysSeeks() {
        assertEquals(Drag.SEEK, drag(80f, 10f).mode)
        // Not twice the vertical move: not a seek yet.
        assertEquals(Drag.NONE, drag(50f, 30f).mode)
    }

    @Test
    fun aFastFlickRightAfterTouchingIsNotASeek() {
        assertEquals(Drag.NONE, drag(80f, 0f, elapsedMs = 50).mode)
    }

    @Test
    fun upAndDownIsBrightnessLeftVolumeRight() {
        assertEquals(Drag.BRIGHTNESS, drag(0f, -60f, startX = 200f).mode)
        assertEquals(Drag.VOLUME, drag(0f, 60f, startX = 800f).mode)
    }

    @Test
    fun holdingSlidesTheSpeed() {
        assertEquals(Drag.SPEED, drag(0f, 0f, holding = true).mode)
    }

    @Test
    fun anOpenSheetSwallowsSwipes() {
        assertEquals(Drag.IGNORE, drag(100f, 0f, sheetOpen = true).mode)
        assertEquals(Drag.NONE, drag(5f, 0f, sheetOpen = true).mode)
    }

    @Test
    fun theSmallPlayerSwipesToFullScreenOrPictureInPicture() {
        assertEquals(PlayerGestures.DragStart(Drag.IGNORE, Swipe.FULLSCREEN), drag(0f, -60f, compact = true))
        assertEquals(PlayerGestures.DragStart(Drag.IGNORE, Swipe.PIP), drag(0f, 60f, compact = true))
        assertEquals(PlayerGestures.DragStart(Drag.IGNORE, Swipe.NONE), drag(100f, 0f, compact = true))
        assertEquals(PlayerGestures.DragStart(Drag.NONE), drag(5f, 5f, compact = true))
    }

    @Test
    fun levelsFollowTheFingerAndStayInRange() {
        assertEquals(0.75f, PlayerGestures.level(0.5f, -200f, 1000f), 0.001f)
        assertEquals(1f, PlayerGestures.level(0.9f, -900f, 1000f), 0f)
        assertEquals(0f, PlayerGestures.level(0.1f, 900f, 1000f), 0f)
    }

    @Test
    fun seekTargetsStayInsideTheVideo() {
        assertEquals(15.0, PlayerGestures.seekTarget(10.0, 100f, 60.0), 1e-9)
        assertEquals(0.0, PlayerGestures.seekTarget(1.0, -100f, 60.0), 0.0)
        assertEquals(60.0, PlayerGestures.seekTarget(59.0, 100f, 60.0), 0.0)
        assertEquals(1010.0, PlayerGestures.seekTarget(1000.0, 200f, 0.0), 1e-9) // length unknown
    }

    @Test
    fun holdSpeedStartsAt2xAndSlides() {
        assertEquals(2.0, PlayerGestures.holdSpeed(0f, 1000f), 0.0)
        assertEquals(4.0, PlayerGestures.holdSpeed(1000f, 1000f), 0.0)
        assertEquals(0.25, PlayerGestures.holdSpeed(-1000f, 1000f), 0.0)
        assertEquals(2.5, PlayerGestures.holdSpeed(50f, 1000f), 0.0)
    }

    private fun scrub(target: Double, ready: Boolean?, since: Long = 1000, seeking: Boolean = false) =
        PlayerGestures.scrub(target, origin = 100.0, position = 100.0, bufferedUntil = 130.0,
            ready = ready, sinceLastSeekMs = since, mpvSeeking = seeking)

    @Test
    fun scrubbingOverTheBufferMovesThePicture() {
        val s = scrub(120.0, ready = false)
        assertEquals(ScrubAction.SEEK_NOW, s.action)
        assertNull(s.note)
        assertEquals("2:00  [+0:20]", s.label)
    }

    @Test
    fun scrubbingOverDownloadedDataMovesThePicture() {
        assertEquals(ScrubAction.SEEK_NOW, scrub(500.0, ready = true).action)
    }

    @Test
    fun scrubbingSeeksAtMostEveryQuarterSecondAndNotDuringASeek() {
        assertEquals(ScrubAction.WAIT, scrub(120.0, ready = true, since = 100).action)
        assertEquals(ScrubAction.WAIT, scrub(120.0, ready = true, seeking = true).action)
    }

    @Test
    fun scrubbingOverMissingDataPrefetches() {
        val s = scrub(500.0, ready = false)
        assertEquals(ScrubAction.PREFETCH, s.action)
        assertEquals("will buffer", s.note)
        assertEquals(ScrubAction.PREFETCH, scrub(500.0, ready = null).action)
        assertNull(scrub(500.0, ready = null).note)
    }

    @Test
    fun scrubLabelShowsBackwardsMoves() {
        assertEquals("1:30  [-0:10]", scrub(90.0, ready = true).label)
        assertEquals("1:00:00  [+58:20]", scrub(3600.0, ready = true).label)
    }
}

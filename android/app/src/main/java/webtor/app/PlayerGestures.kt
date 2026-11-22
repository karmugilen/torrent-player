package webtor.app

/**
 * What a finger does on the built-in player: tap sides, swipe modes, levels, seek
 * targets, hold speed, and seeking while scrubbing. Pure numbers in, decisions out,
 * so every rule has a unit test (PlayerGesturesTest); PlayerScreen feeds positions
 * and times and carries the decisions out. Pixel values are raw touch pixels.
 */
object PlayerGestures {
    /** A finger has to move this far before a swipe counts. */
    const val SLOP_PX = 30f
    /** Holding this long plays at 2x. */
    const val HOLD_MS = 450L
    /** Taps on the same side within this time keep adding to a skip streak. */
    const val STREAK_MS = 800L
    const val SKIP_SECONDS = 10
    /** Sideways seeking: seconds per pixel, as in mpvEx. */
    const val SEEK_SECONDS_PER_PX = 0.05
    /** A sideways move this soon after touching down is a fling, not a seek. */
    const val SEEK_MIN_MS = 100L
    /** While scrubbing over ready data, move the picture at most this often. */
    const val LIVE_SEEK_EVERY_MS = 250L
    val SPEEDS = listOf(0.25, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0)

    enum class Drag { NONE, VOLUME, BRIGHTNESS, SEEK, SPEED, IGNORE }

    /** Small player on the page: swipe up goes full screen, swipe down to picture in picture. */
    enum class Swipe { NONE, FULLSCREEN, PIP }

    data class DragStart(val mode: Drag, val swipe: Swipe = Swipe.NONE)

    /** -1 left third, 1 right third, 0 middle. */
    fun side(x: Float, width: Float): Int = when {
        x < width / 3f -> -1
        x > width * 2f / 3f -> 1
        else -> 0
    }

    /**
     * What a moving finger becomes, measured from where it went down. Clearly sideways
     * (over [SLOP_PX] and twice the vertical move) seeks; up/down is brightness on the
     * left half, volume on the right. NONE means not decided yet.
     */
    fun dragStart(
        dx: Float, dy: Float, elapsedMs: Long, startX: Float, width: Float,
        holding: Boolean, sheetOpen: Boolean, compact: Boolean,
    ): DragStart {
        val ax = kotlin.math.abs(dx)
        val ay = kotlin.math.abs(dy)
        val moved = ax > SLOP_PX || ay > SLOP_PX
        return when {
            holding -> DragStart(Drag.SPEED)
            sheetOpen -> DragStart(if (moved) Drag.IGNORE else Drag.NONE)
            // The small player has no brightness, volume or drag-seek: the page scrolls instead.
            compact -> when {
                !moved -> DragStart(Drag.NONE)
                dy < -SLOP_PX && ay > ax -> DragStart(Drag.IGNORE, Swipe.FULLSCREEN)
                dy > SLOP_PX && ay > ax -> DragStart(Drag.IGNORE, Swipe.PIP)
                else -> DragStart(Drag.IGNORE)
            }
            ax > SLOP_PX && ax > ay * 2f && elapsedMs > SEEK_MIN_MS -> DragStart(Drag.SEEK)
            ay > SLOP_PX && ay > ax -> DragStart(if (startX < width / 2f) Drag.BRIGHTNESS else Drag.VOLUME)
            else -> DragStart(Drag.NONE)
        }
    }

    /** Volume or brightness after moving [dy] from [start]; 80% of the height spans all of it. */
    fun level(start: Float, dy: Float, height: Float): Float =
        (start - dy / (height * 0.8f)).coerceIn(0f, 1f)

    /** Where a sideways seek lands, kept inside the video. */
    fun seekTarget(origin: Double, dx: Float, duration: Double): Double =
        (origin + dx * SEEK_SECONDS_PER_PX).coerceIn(0.0, if (duration > 0) duration else Double.MAX_VALUE)

    /** Hold speed after sliding [dx] sideways: right is faster; about a third of the width spans all steps. */
    fun holdSpeed(dx: Float, width: Float): Double {
        val step = (dx / width * (SPEEDS.size - 1) * 3.5f).toInt()
        return SPEEDS[(SPEEDS.indexOf(2.0) + step).coerceIn(0, SPEEDS.size - 1)]
    }

    enum class ScrubAction {
        /** The spot is ready: move the picture there now. */
        SEEK_NOW,
        /** Ready, but a seek just went out: skip this one. */
        WAIT,
        /** Not downloaded: ask the engine for it, seek on release. */
        PREFETCH,
    }

    data class Scrub(val label: String, val note: String?, val action: ScrubAction)

    /**
     * While dragging: show where it lands and move the picture like a normal player
     * when that spot is downloaded ([ready]) or in mpv's buffer. Other spots are only
     * prefetched, then seeked once on release.
     */
    fun scrub(
        target: Double, origin: Double, position: Double, bufferedUntil: Double,
        ready: Boolean?, sinceLastSeekMs: Long, mpvSeeking: Boolean,
    ): Scrub {
        val delta = target - origin
        val label = clock(target) + "  [" + (if (delta >= 0) "+" else "-") + clock(kotlin.math.abs(delta)) + "]"
        val inBuffer = target >= position - 5 && target <= bufferedUntil
        val note = if (ready == false && !inBuffer) "will buffer" else null
        val action = when {
            ready != true && !inBuffer -> ScrubAction.PREFETCH
            sinceLastSeekMs > LIVE_SEEK_EVERY_MS && !mpvSeeking -> ScrubAction.SEEK_NOW
            else -> ScrubAction.WAIT
        }
        return Scrub(label, note, action)
    }

    fun clock(seconds: Double): String {
        val s = seconds.toLong().coerceAtLeast(0)
        return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
    }
}

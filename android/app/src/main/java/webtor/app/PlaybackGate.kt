package webtor.app

/**
 * When the built-in player may play, and when it shows loading.
 *
 * Owns every wait the player has: opening a file, mpv waiting for data, a seek in
 * flight, and the length wait (held paused until the total length is known, so
 * playback never starts with no end time; no time limit, because a torrent may need
 * a while to fetch the header or the index at the end of the file).
 *
 * The player forwards mpv's events here and carries out the [Command]s it gets back.
 * Nothing in here touches mpv, so every rule has a unit test (PlaybackGateTest).
 * Safe to call from mpv's event thread and the UI thread; [onChange] gets each new [View].
 */
class PlaybackGate(private val onChange: (View) -> Unit = {}) {

    /** What the screen needs. */
    data class View(
        /** Waiting for something: opening, data, a seek, or the length. */
        val buffering: Boolean = true,
        /** Held paused until the length is known. */
        val waitingForLength: Boolean = false,
        /** A seek the app asked for that mpv has not finished yet. */
        val seekPending: Boolean = false,
        /** mpv is seeking right now (its own `seeking` property). */
        val mpvSeeking: Boolean = false,
    ) {
        /** Loading spinner: waiting while playing or for the length, or a seek in flight (not while the finger scrubs). */
        fun spinning(paused: Boolean, scrubbing: Boolean): Boolean =
            (buffering && (!paused || waitingForLength)) || (seekPending && !scrubbing)
    }

    enum class Command { PAUSE, PLAY }

    private var loading = true
    private var cacheWait = false
    private var seekWait = false
    private var lengthWait = false
    /** The file is open and mpv has no length for it at all: only then may play skip the wait. */
    private var openedWithoutLength = false
    private var seekPending = false

    @Volatile var view = View(); private set

    /** mpv is created paused: hold until the first file's length is known. */
    @Synchronized fun engineStarting() { lengthWait = true; publish() }

    /**
     * A file starts: pause before mpv gets far with it, so the next episode never
     * plays a moment before its length is known.
     */
    @Synchronized fun fileStarting(): List<Command> {
        openedWithoutLength = false
        loading = true
        seekPending = false
        lengthWait = true
        publish()
        return listOf(Command.PAUSE)
    }

    /** The app asked mpv to load another episode. */
    @Synchronized fun switching() { loading = true; publish() }

    /** mpv opened the file. */
    @Synchronized fun fileLoaded() { loading = false; publish() }

    /**
     * The length mpv had right after opening. mpv only reports a length when it
     * changes, so an episode as long as the last one may never send one: read it once.
     */
    @Synchronized fun lengthAtOpen(seconds: Double?): List<Command> {
        openedWithoutLength = seconds == null || seconds <= 0
        return if (openedWithoutLength) emptyList() else lengthKnown(seconds!!)
    }

    /** mpv reported the length: the wait ends and playback starts. */
    @Synchronized fun lengthKnown(seconds: Double): List<Command> {
        if (seconds <= 0 || !lengthWait) return emptyList()
        lengthWait = false
        publish()
        return listOf(Command.PLAY)
    }

    @Synchronized fun cacheWait(on: Boolean) { cacheWait = on; publish() }
    @Synchronized fun mpvSeeking(on: Boolean) { seekWait = on; publish() }
    @Synchronized fun seekRequested() { seekPending = true; publish() }
    /** Never spin forever if mpv drops a seek. */
    @Synchronized fun seekGaveUp() { seekPending = false; publish() }

    /** mpv shows frames again after opening or a seek. */
    @Synchronized fun playbackRestarted() {
        loading = false; seekWait = false; seekPending = false
        publish()
    }

    /**
     * Play/pause pressed. Returns null when it is a normal toggle. During the length
     * wait play does nothing, unless the file opened with no length at all (it would
     * never come): then the wait ends and playback starts.
     */
    @Synchronized fun playPressed(): List<Command>? {
        if (!lengthWait) return null
        if (!openedWithoutLength) return emptyList()
        lengthWait = false
        publish()
        return listOf(Command.PLAY)
    }

    /** The screen went away: drop the wait without starting playback. */
    @Synchronized fun stopped() { if (lengthWait) { lengthWait = false; publish() } }

    private fun publish() {
        val next = View(
            buffering = loading || cacheWait || seekWait || lengthWait,
            waitingForLength = lengthWait,
            seekPending = seekPending,
            mpvSeeking = seekWait,
        )
        if (next != view) { view = next; onChange(next) }
    }
}

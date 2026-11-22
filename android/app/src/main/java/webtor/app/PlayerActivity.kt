package webtor.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.ActivityInfo
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material.icons.filled.AspectRatio
import androidx.compose.material.icons.filled.ScreenLockRotation
import androidx.compose.material.icons.filled.ScreenRotation
import androidx.compose.material.icons.filled.Audiotrack
import androidx.compose.material.icons.filled.Forward10
import androidx.compose.material.icons.filled.FastForward
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.FastRewind
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PictureInPictureAlt
import androidx.compose.material.icons.filled.Replay10
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material.icons.filled.Subtitles
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.Fullscreen
import androidx.compose.material.icons.filled.FullscreenExit
import androidx.compose.runtime.collectAsState
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.layout.layout
import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Brush
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.foundation.layout.height
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.text.font.FontWeight
import dev.jdtech.mpv.MPVLib
import kotlinx.coroutines.Dispatchers
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.concurrent.read
import kotlin.concurrent.write
import kotlin.math.roundToInt

/** Picture size in the built-in player, remembered between videos. */
enum class PlayerAspect(val label: String, val detail: String) {
    FIT("Best fit", "Whole picture, bars if needed"), CROP("Crop", "Fill the screen, edges cut"),
    STRETCH("Stretch", "Fill the screen, shape ignored"), R16_9("16:9", "Show as widescreen"),
    R4_3("4:3", "Show as classic TV");

    fun next(): PlayerAspect = entries[(ordinal + 1) % entries.size]

    companion object {
        private const val KEY = "playerAspect"
        fun load(context: Context): PlayerAspect =
            context.getSharedPreferences("settings", Context.MODE_PRIVATE).getString(KEY, null)
                ?.let { name -> entries.firstOrNull { it.name == name } } ?: FIT
        fun save(context: Context, value: PlayerAspect) {
            context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putString(KEY, value.name).apply()
        }
    }
}

/** Settings flag for the built-in mpv player (experimental, on by default in this test build). */
object BuiltInPlayer {
    private const val KEY = "builtInPlayer"
    fun enabled(context: Context): Boolean =
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).getBoolean(KEY, true)
    /** False until the user picked built-in or external once (asked on first open). */
    fun chosen(context: Context): Boolean =
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).contains(KEY)
    fun setEnabled(context: Context, value: Boolean) {
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putBoolean(KEY, value).apply()
    }
    // Off by default, as in mpvEx.
    fun rememberBrightness(context: Context): Boolean =
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).getBoolean("rememberBrightness", false)
    fun setRememberBrightness(context: Context, value: Boolean) {
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putBoolean("rememberBrightness", value).apply()
    }
    /** Level last set in the player, or -1 when none was saved. */
    fun savedBrightness(context: Context): Float =
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).getFloat("savedBrightness", -1f)
    fun setSavedBrightness(context: Context, value: Float) {
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putFloat("savedBrightness", value).apply()
    }
}

/**
 * Built-in player on libmpv. It accepts the same intent the app sends to
 * external players (data, position, video_list, subs) and returns the same
 * result (data, position, duration), so resume and leases work unchanged.
 */
class PlayerActivity : ComponentActivity(), MPVLib.EventObserver {
    companion object {
        /** The libmpv instance is process-wide; only its current owner may tear it down. */
        @Volatile private var owner: PlayerActivity? = null
        /** Calls share it; only create and destroy take it alone, so one slow call never blocks another. */
        private val lock = java.util.concurrent.locks.ReentrantReadWriteLock()
        private val commands = java.util.concurrent.Executors.newSingleThreadExecutor { r -> Thread(r, "mpv-commands") }

        /**
         * Tapping a torrent whose video is playing in the small window brings that player
         * back full size, still playing, with its page, like YouTube. False when there is none.
         */
        /** Page to show under the next player when one page player hands over to another. */
        @Volatile private var nextDetailKey: String? = null
        @Volatile private var nextDetailAt = 0L
        /** The next player started within a few seconds shows [key]'s page under the video. */
        /** A live player shows torrent [key]. */
        fun playing(key: String): Boolean = owner?.let { it.torrentKey == key && it.alive && !it.finishing } == true
        fun showPageFor(key: String) { nextDetailKey = key; nextDetailAt = android.os.SystemClock.elapsedRealtime() }
        fun takeNextDetailKey(): String? {
            val key = nextDetailKey; nextDetailKey = null
            return key?.takeIf { android.os.SystemClock.elapsedRealtime() - nextDetailAt < 30_000 }
        }

        fun expandIfPlaying(context: Context, key: String): Boolean {
            val p = owner ?: return false
            if (p.detailKey != key || !p.alive || p.finishing || p.isFinishing || !p.inPip.value) return false
            android.util.Log.i("webtor-player", "card tap: expanding the small window for $key")
            // Started from itself, inside its own small-window task: Android expands that task
            // to full screen and hands the intent to the running player (singleTop), no reload.
            val ok = runCatching {
                p.startActivity(Intent(p, PlayerActivity::class.java)
                    .putExtra("expand_only", true)
                    .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT))
            }.onFailure { android.util.Log.w("webtor-player", "expand from small window failed", it) }.isSuccess
            if (!ok) {
                val am = context.getSystemService(Context.ACTIVITY_SERVICE) as android.app.ActivityManager
                return runCatching { am.moveTaskToFront(p.taskId, 0) }.isSuccess
            }
            return true
        }
    }

    @Volatile private var alive = false

    /** Run [block] on mpv only while this activity owns a live instance (else null). Not from mpv's own callbacks. */
    private inline fun <T> mpv(block: () -> T): T? = lock.read { if (alive) block() else null }

    /**
     * Same as [mpv] but off the UI thread, in order. Buttons and gestures use this:
     * mpv commands wait for the player core, which can be busy reading a slow stream,
     * and waiting on the UI thread made taps look dead.
     */
    private fun mpvSend(block: () -> Unit) { commands.execute { mpv(block) } }

    /** Release the player once. Detach the picture first so no render thread touches a freed context. */
    private fun teardown() {
        loadJob?.cancel()
        lock.write {
            if (!alive) return
            alive = false
            // Waits for any callback in progress, so none runs for this activity afterwards.
            runCatching { MPVLib.removeObserver(this) }
            runCatching { MPVLib.setPropertyString("vo", "null") }
            runCatching { MPVLib.setOptionString("force-window", "no") }
            runCatching { MPVLib.detachSurface() }
            runCatching { MPVLib.destroy() }
            if (owner === this) owner = null
        }
    }

    private data class Item(val uri: Uri, val name: String)

    private var items: List<Item> = emptyList()
    private var subtitles: List<Uri> = emptyList()
    private var startSeconds = 0.0
    private var firstFileLoaded = false
    /** Playlist place of the video picked to play; its sidecar subtitles come with the intent. */
    private var startIndex = 0
    /** Library key of the torrent playing, for per-torrent audio and subtitle choices. */
    private var torrentKey: String? = null
    /** Episode end reached and handled (saved, card shown or player closed). */
    private var endHandled = false
    /** Playlist place of the next episode while the "Up next" card shows. */
    private val nextCard = mutableStateOf<Int?>(null)
    private var finishing = false

    /**
     * Opened from a torrent's page: portrait shows the video at the top with the page
     * below it, like YouTube; landscape or the fullscreen button makes the same player
     * fill the screen (one mpv, nothing reloads).
     */
    private var detailKey: String? = null
    private val pageMode get() = detailKey != null
    /** After the fullscreen buttons force a side, follow the sensor again once the phone is turned that way: 1 portrait, 2 landscape. */
    @Volatile private var unlockWhen = 0
    private val orientationListener by lazy {
        object : android.view.OrientationEventListener(this) {
            override fun onOrientationChanged(deg: Int) {
                val want = unlockWhen
                if (deg < 0 || want == 0) return
                val portrait = deg < 25 || deg > 335
                val land = deg in 65..115 || deg in 245..295
                if ((want == 1 && portrait) || (want == 2 && land && autoRotate())) {
                    unlockWhen = 0
                    runOnUiThread { if (!rotationLocked.value) requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_USER }
                }
            }
        }
    }
    private fun autoRotate(): Boolean =
        android.provider.Settings.System.getInt(contentResolver, android.provider.Settings.System.ACCELEROMETER_ROTATION, 0) == 1
    private fun isLandscape() = resources.configuration.orientation == android.content.res.Configuration.ORIENTATION_LANDSCAPE

    private fun setFullscreen(on: Boolean) {
        rotationLocked.value = false
        unlockWhen = if (on) 2 else 1
        requestedOrientation = if (on) ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE else ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
    }

    /** Back: fullscreen goes back to the page; the page closes the video and goes to the library. */
    private fun back() {
        if (!pageMode) return close()
        if (isLandscape()) { setFullscreen(false); return }
        // The page and the video leave together, like leaving a detail page.
        (application as WebtorApp).session.closeFiles()
        close()
    }

    /** A file tapped in the page below the video: switch in place when it is in this playlist. */
    private fun playFile(index: Int) {
        val app = application as WebtorApp
        val entry = app.session.ui.value.library.find { it.key == detailKey } ?: return
        val name = entry.files.find { it.index == index }?.name
        val at = items.indexOfFirst { torrentOf(it.uri)?.second == index || (name != null && it.name == name) }
        if (at >= 0) {
            switchTo(at)
            return
        }
        detailKey?.let { showPageFor(it) }
        close()
        app.session.play(entry, index)
    }

    private val title = mutableStateOf("")
    private val position = mutableDoubleStateOf(0.0)
    private val duration = mutableDoubleStateOf(0.0)
    private val cached = mutableDoubleStateOf(0.0)
    private val paused = mutableStateOf(false)
    private val buffering = mutableStateOf(true)
    @Volatile private var loading = true
    @Volatile private var cacheWait = false
    @Volatile private var seekWait = false
    /** Held paused until the total length is known, so playback never starts with no end time. */
    private val lengthWait = mutableStateOf(false)
    /** The file is open and mpv has no length for it at all: only then may play skip the wait. */
    @Volatile private var openedWithoutLength = false
    /** A seek the app asked for and mpv has not finished yet; shows the spinner even while paused. */
    private val seekPending = mutableStateOf(false)
    private fun markSeek() { seekPending.value = true }
    private fun updateBuffering() { buffering.value = loading || cacheWait || seekWait || lengthWait.value }

    /**
     * Start of each file: pause until mpv reports the duration. No time limit: a torrent
     * may need a while to fetch the header or the index at the end of the file, and
     * starting before that leaves the player with no end time. Play does not skip the wait,
     * unless the file opened with no length at all (it would never come).
     * Runs on the UI thread, like every duration update, so a late reset can never wipe
     * a length that already arrived.
     */
    private fun holdForLength() {
        duration.doubleValue = 0.0
        lengthWait.value = true; updateBuffering()
        mpvSend { MPVLib.setPropertyBoolean("pause", true) }
    }
    private var lengthReported: Uri? = null
    /** Tell the engine once per file that the length is known, so it stops fetching the index first. */
    private fun reportLength() {
        val uri = items.getOrNull(playlistPos.intValue)?.uri ?: return
        if (uri == lengthReported) return
        lengthReported = uri
        val (id, index) = torrentOf(uri) ?: return
        lifecycleScope.launch(Dispatchers.IO) { runCatching { (application as WebtorApp).engineClient.lengthKnown(id, index) } }
    }
    private fun releaseLength(play: Boolean) {
        if (!lengthWait.value) return
        lengthWait.value = false; updateBuffering()
        if (play) mpvSend { MPVLib.setPropertyBoolean("pause", false) }
    }
    private val playlistPos = mutableIntStateOf(0)

    /** True when the latest touch began on a system gesture edge (home pill, status bar). */
    private var edgeTouch = false

    /**
     * Swiping up from the home pill or down from the top belongs to the system.
     * The gesture layer ignores touches that start there, so they cannot seek,
     * change volume or pop up the time overlay; buttons there still work.
     */
    override fun dispatchTouchEvent(ev: android.view.MotionEvent): Boolean {
        if (ev.actionMasked == android.view.MotionEvent.ACTION_DOWN) {
            val h = window.decorView.height.toFloat()
            val minEdge = 40 * resources.displayMetrics.density
            val g = androidx.core.view.ViewCompat.getRootWindowInsets(window.decorView)
                ?.getInsetsIgnoringVisibility(WindowInsetsCompat.Type.systemGestures())
            val bottom = maxOf(minEdge, (g?.bottom ?: 0).toFloat())
            val top = maxOf(minEdge * 0.6f, (g?.top ?: 0).toFloat())
            edgeTouch = h > 0 && (ev.y >= h - bottom || ev.y <= top)
        }
        // Buttons near the edges still get the touch; only the gesture layer skips it.
        return super.dispatchTouchEvent(ev)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        detailKey = intent.getStringExtra("detail_key")
        if (BuiltInPlayer.rememberBrightness(this)) BuiltInPlayer.savedBrightness(this).takeIf { it >= 0f }?.let { saved ->
            window.attributes = window.attributes.apply { screenBrightness = saved }
        }
        requestedOrientation = if (pageMode) ActivityInfo.SCREEN_ORIENTATION_USER else ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        if (pageMode) orientationListener.takeIf { it.canDetectOrientation() }?.enable()
        if (pageMode && Build.VERSION.SDK_INT >= 34) {
            // In and out with no slide: the page player stands in for the detail page.
            overrideActivityTransition(OVERRIDE_TRANSITION_OPEN, 0, 0)
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        if (Build.VERSION.SDK_INT >= 28) {
            window.attributes.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }

        readIntent(intent)
        if (isFinishing) return // no video to play
        startMpv()
        androidx.core.content.ContextCompat.registerReceiver(this, pipReceiver, android.content.IntentFilter(pipAction),
            androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED)
        // While the small window plays over the library, keep the card's "Left off" current.
        lifecycleScope.launch {
            while (alive && !finishing) {
                kotlinx.coroutines.delay(10_000)
                if (inPip.value && !paused.value) savePosition()
            }
        }

        setContent {
            val app = application as WebtorApp
            // Only the theme here: the whole library state changes every second with download
            // stats, and the player (most of all in picture in picture) must not redraw for it.
            val look by remember {
                app.session.ui.map { Look(it.darkTheme, it.appTheme, it.reducedEffects, it.dynamicColors) }
                    .distinctUntilChanged()
            }.collectAsState(app.session.ui.value.let { Look(it.darkTheme, it.appTheme, it.reducedEffects, it.dynamicColors) })
            val landscape = androidx.compose.ui.platform.LocalConfiguration.current.orientation ==
                android.content.res.Configuration.ORIENTATION_LANDSCAPE
            val full = !pageMode || landscape || inPip.value
            LaunchedEffect(full) {
                WindowInsetsControllerCompat(window, window.decorView).apply {
                    if (full) hide(WindowInsetsCompat.Type.systemBars()) else show(WindowInsetsCompat.Type.systemBars())
                }
            }
            GaleTheme(dark = look.dark, theme = look.theme, reducedEffects = look.reduced, dynamicColors = look.dynamic) {
                // How far the page has pushed the video up: 0 (all shown) down to minus its height.
                // The video scrolls away with the page and comes back when the list is at its top.
                val shift = remember { androidx.compose.runtime.mutableFloatStateOf(0f) }
                LaunchedEffect(full) { shift.floatValue = 0f }
                androidx.compose.foundation.layout.BoxWithConstraints(
                    Modifier.fillMaxSize()
                        .background(if (full) Color.Black else MaterialTheme.colorScheme.background)
                        .then(if (full) Modifier else Modifier.background(Color.Black).statusBarsPadding())
                        .clipToBounds(),
                ) {
                    val playerH = constraints.maxWidth * 9f / 16f
                    val scroll = remember(playerH) {
                        object : androidx.compose.ui.input.nestedscroll.NestedScrollConnection {
                            override fun onPreScroll(available: androidx.compose.ui.geometry.Offset,
                                source: androidx.compose.ui.input.nestedscroll.NestedScrollSource): androidx.compose.ui.geometry.Offset {
                                if (available.y >= 0) return androidx.compose.ui.geometry.Offset.Zero
                                val next = (shift.floatValue + available.y).coerceAtLeast(-playerH)
                                val used = next - shift.floatValue; shift.floatValue = next
                                return androidx.compose.ui.geometry.Offset(0f, used)
                            }
                            override fun onPostScroll(consumed: androidx.compose.ui.geometry.Offset, available: androidx.compose.ui.geometry.Offset,
                                source: androidx.compose.ui.input.nestedscroll.NestedScrollSource): androidx.compose.ui.geometry.Offset {
                                if (available.y <= 0) return androidx.compose.ui.geometry.Offset.Zero
                                val next = (shift.floatValue + available.y).coerceAtMost(0f)
                                val used = next - shift.floatValue; shift.floatValue = next
                                return androidx.compose.ui.geometry.Offset(0f, used)
                            }
                        }
                    }
                    Column(Modifier.fillMaxSize().then(if (full) Modifier else Modifier.nestedScroll(scroll))) {
                    // Same node in both layouts, so the video surface is kept, never rebuilt.
                    // On the page its slot shrinks as the page scrolls and the video slides up out of it.
                    Box(if (full) Modifier.fillMaxSize() else Modifier.fillMaxWidth()
                        .layout { m, c ->
                            val h = playerH.toInt()
                            val p = m.measure(c.copy(minHeight = h, maxHeight = h))
                            val shown = (playerH + shift.floatValue).toInt().coerceIn(0, h)
                            layout(p.width, shown) { p.place(0, shown - h) }
                        }
                        .clipToBounds()) {
                        androidx.compose.material3.MaterialTheme(colorScheme = androidx.compose.material3.darkColorScheme(
                            primary = Color.White, onPrimary = Color.Black,
                        )) { PlayerScreen(
                            surface = { SurfaceView(it).apply {
                                holder.addCallback(surfaceCallback)
                                videoView = this
                                // Keep the PiP hint on the video's real place on screen.
                                addOnLayoutChangeListener { _, l, t, r, b, ol, ot, or, ob ->
                                    if (l != ol || t != ot || r != or || b != ob) updatePip()
                                }
                            } },
                            onBack = ::back,
                            compact = !full,
                        ) }
                    }
                    if (!full) {
                        // The page below the video reads the full state; it is gone in fullscreen and PiP.
                        val ui by app.session.ui.collectAsState()
                        val entry = ui.library.find { it.key == detailKey }
                        Box(Modifier.fillMaxWidth().weight(1f).background(MaterialTheme.colorScheme.background)) {
                            if (entry != null) TorrentFilesScreen(
                                entry = entry,
                                error = ui.error,
                                onBack = ::back,
                                onPlay = ::playFile,
                                onToggleFile = { index -> app.session.toggleDownloadFile(entry.key, index) },
                                onPause = { app.session.pause(entry) },
                                onResume = { app.session.resume(entry) },
                                onDrop = { app.session.drop(entry) },
                                onDelete = { },
                                embedded = true,
                                playingIndex = items.getOrNull(playlistPos.intValue)?.let { fileOf(it)?.second?.index },
                            )
                        }
                    }
                    }
                }
            }
        }
        updatePip() // so leaving the app goes straight to picture in picture
    }

    @Suppress("DEPRECATION")
    private fun readIntent(intent: Intent) {
        val data = intent.data ?: run { finish(); return }
        val name = intent.getStringExtra("title") ?: data.lastPathSegment.orEmpty()
        val list = intent.getParcelableArrayExtra("video_list")?.mapNotNull { it as? Uri }.orEmpty()
        val names = intent.getStringArrayExtra("video_list.name")
        items = if (list.isEmpty()) listOf(Item(data, name)) else {
            val all = list.mapIndexed { i, u -> Item(u, names?.getOrNull(i) ?: u.lastPathSegment.orEmpty()) }
            // The whole run of episodes, starting at the chosen one, so Previous works too.
            val at = all.indexOfFirst { it.uri == data }
            if (at < 0) listOf(Item(data, name)) else { startIndex = at; all }
        }
        subtitles = intent.getParcelableArrayExtra("subs")?.mapNotNull { it as? Uri }.orEmpty()
        startSeconds = (intent.extras?.get("position") as? Number)?.toLong()?.div(1000.0) ?: 0.0
        torrentKey = intent.getStringExtra("detail_key") ?: intent.getStringExtra("entry_key")
        playlistPos.intValue = startIndex
        title.value = items[startIndex].name
    }

    /** mpv opens http directly; content URIs go through a descriptor mpv closes itself. */
    /**
     * External subtitles, fetched to a local file first, each given to mpv once whole.
     * mpv loads a subtitle in one go and does nothing else meanwhile: a torrent .srt not
     * downloaded yet held the first frame, play and seek for 40 s.
     */
    private fun addSubtitles(subs: List<Uri>) {
        if (subs.isEmpty()) return
        val episode = playlistPos.intValue
        lifecycleScope.launch(Dispatchers.IO) {
            // A kept subtitle choice picks the track once all are in; else the first file is shown.
            val kept = trackPref("sub") != null
            val dir = java.io.File(cacheDir, "player-subs").apply { deleteRecursively(); mkdirs() }
            subs.forEachIndexed { i, uri ->
                val name = (uri.lastPathSegment ?: "sub$i").substringAfterLast('/').ifBlank { "sub$i" }
                val file = java.io.File(dir, "$i-" + name.replace(Regex("[^A-Za-z0-9._-]"), "_"))
                val ok = runCatching {
                    val stream = if (uri.scheme == "http" || uri.scheme == "https") java.net.URL(uri.toString()).openStream()
                        else if (PlaybackProvider.isOwn(uri)) PlaybackProvider.openInApp(application as WebtorApp, uri)
                        else contentResolver.openInputStream(uri)
                    stream?.use { input -> file.outputStream().use { input.copyTo(it) } } != null
                }.getOrDefault(false)
                if (!ok || !alive || episode != playlistPos.intValue) return@forEachIndexed
                mpvSend { MPVLib.command(arrayOf("sub-add", file.path, if (i == 0 && !kept) "select" else "auto")) }
            }
            if (kept && alive && episode == playlistPos.intValue) applyTrackPrefs(listOf("sub"))
        }
    }

    private fun mpvPath(uri: Uri): String = when (uri.scheme) {
        "content" -> try {
            contentResolver.openFileDescriptor(uri, "r")?.let { "fdclose://${it.detachFd()}" } ?: uri.toString()
        } catch (_: Exception) { uri.toString() }
        else -> uri.toString()
    }

    private fun startMpv() {
        lock.write {
            // A previous player still open (e.g. left in picture in picture) must let go first.
            // Close it as well: with its mpv gone it could only show a dead, frozen window.
            owner?.takeIf { it !== this }?.let { if (it.isFinishing) it.teardown() else it.finish() }
            MPVLib.create(applicationContext)
            owner = this
            alive = true
        }
        // libass on Android has no system font lookup: with no font, picked subtitles
        // draw nothing. mpv falls back to ~~/subfont.ttf, so give it one (as mpv-android).
        val mpvDir = subtitleFontDir()
        MPVLib.setOptionString("config", "yes")
        MPVLib.setOptionString("config-dir", mpvDir.path)
        listOf(
            // Same base as mpvEx: fast profile, hardware decode with software fallback.
            "profile" to "fast",
            // No picture output until the surface is attached; opening a file with no
            // surface aborts the whole app (vo_android_init asserts a window id).
            "vo" to "null",
            "gpu-context" to "android",
            "opengl-es" to "yes",
            "hwdec" to "mediacodec,mediacodec-copy,no",
            "hwdec-codecs" to "all",
            "vd-lavc-film-grain" to "cpu",
            "ao" to "audiotrack,opensles",
            "audio-set-media-role" to "yes",
            "tls-verify" to "no",
            "cache" to "yes",
            // The torrent engine already holds the data on the phone; mpv needs only a short
            // read-ahead of its own, not a second large copy in memory.
            "demuxer-max-bytes" to "32MiB",
            "demuxer-max-back-bytes" to "16MiB",
            "hr-seek" to "no",
            "hr-seek-framedrop" to "yes",
            "cache-pause-initial" to "no",
            "network-timeout" to "0",
            // Stay on the last frame at the end: the app decides what comes next (Up next card).
            "keep-open" to "yes",
            "idle" to "yes",
            "force-window" to "no",
            "save-position-on-quit" to "no",
            "input-default-bindings" to "yes",
            "sub-auto" to "fuzzy",
            "sub-scale-with-window" to "yes",
            "osd-level" to "1",
            "osd-font-size" to "32",
            "screenshot-format" to "jpg",
            "screenshot-jpeg-quality" to "80",
        ).forEach { (k, v) -> MPVLib.setOptionString(k, v) }
        MPVLib.setOptionString("pause", "yes") // until the length is known
        lengthWait.value = true
        MPVLib.init()
        applyAspect(aspect.value)
        MPVLib.addObserver(this)
        listOf(
            "time-pos" to MPVLib.MPV_FORMAT_DOUBLE,
            "duration" to MPVLib.MPV_FORMAT_DOUBLE,
            "video-params/aspect" to MPVLib.MPV_FORMAT_DOUBLE,
            "video-params/dw" to MPVLib.MPV_FORMAT_INT64,
            "video-params/dh" to MPVLib.MPV_FORMAT_INT64,
            "demuxer-cache-time" to MPVLib.MPV_FORMAT_DOUBLE,
            "pause" to MPVLib.MPV_FORMAT_FLAG,
            "paused-for-cache" to MPVLib.MPV_FORMAT_FLAG,
            "seeking" to MPVLib.MPV_FORMAT_FLAG,
            "idle-active" to MPVLib.MPV_FORMAT_FLAG,
            "eof-reached" to MPVLib.MPV_FORMAT_FLAG,
        ).forEach { (k, f) -> MPVLib.observeProperty(k, f) }

        if (startSeconds > 1) MPVLib.setPropertyString("start", "%.3f".format(java.util.Locale.ROOT, startSeconds))
    }

    /**
     * mpv's config folder with subfont.ttf: a copy of the phone's own sans font
     * (nothing bundled). Done once; later starts reuse the file.
     */
    private fun subtitleFontDir(): java.io.File {
        val dir = java.io.File(filesDir, "mpv").apply { mkdirs() }
        val font = java.io.File(dir, "subfont.ttf")
        if (font.length() > 0L) return dir
        val fonts = java.io.File("/system/fonts")
        val pick = listOf("Roboto-Regular.ttf", "NotoSans-Regular.ttf", "DroidSans.ttf", "Roboto-Medium.ttf")
            .map { java.io.File(fonts, it) }.firstOrNull { it.isFile && it.length() > 0L }
            ?: fonts.listFiles { f -> f.name.endsWith(".ttf", true) && f.name.contains("Sans", true) }?.firstOrNull()
            ?: fonts.listFiles { f -> f.name.endsWith(".ttf", true) }?.firstOrNull()
        if (pick != null) runCatching {
            val tmp = java.io.File(dir, "subfont.ttf.tmp")
            pick.copyTo(tmp, overwrite = true)
            tmp.renameTo(font)
        }.onFailure { android.util.Log.w("webtor-player", "No subtitle font", it) }
        return dir
    }

    private var filesLoaded = false

    /** Open the files once a surface is attached. Off the main thread, as mpvEx does, so a slow stream never freezes the UI. */
    private var loadJob: kotlinx.coroutines.Job? = null
    private fun loadFilesOnce() {
        if (filesLoaded) return
        filesLoaded = true
        // One episode at a time in mpv; the app keeps the run and moves through it (switchTo).
        val uri = items[playlistPos.intValue].uri
        loadJob = lifecycleScope.launch(Dispatchers.Default) {
            if (!alive) return@launch
            val path = mpvPath(uri)
            // Under the lock: the activity may be torn down on the main thread meanwhile.
            mpv { MPVLib.command(arrayOf("loadfile", path, "replace")) }
        }
    }

    private val surfaceCallback = object : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) {
            mpv {
                MPVLib.attachSurface(holder.surface)
                MPVLib.setOptionString("force-window", "yes")
                MPVLib.setPropertyString("vo", "gpu")
            } ?: return
            loadFilesOnce()
        }
        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
            mpv { MPVLib.setPropertyString("android-surface-size", "${width}x$height") }
        }
        override fun surfaceDestroyed(holder: SurfaceHolder) {
            mpv {
                MPVLib.setPropertyString("vo", "null")
                MPVLib.setOptionString("force-window", "no")
                MPVLib.detachSurface()
            }
        }
    }

    // mpv calls these on its own thread.
    override fun eventProperty(property: String) {}
    override fun eventProperty(property: String, value: Long) {
        if (!alive) return
        when (property) {
            "video-params/dw" -> videoW = value.toInt()
            "video-params/dh" -> videoH = value.toInt()
        }
    }
    override fun eventProperty(property: String, value: Double) {
        if (!alive) return
        when (property) {
            "video-params/aspect" -> videoAspect = value
            // mpv reports this every frame; redrawing the controls that often costs CPU,
            // most of all in picture in picture where they are hidden. Small steps are enough.
            "time-pos" -> {
                val step = if (inPip.value) 1.0 else 0.25
                if (kotlin.math.abs(value - position.doubleValue) >= step) position.doubleValue = value
            }
            "duration" -> runOnUiThread {
                if (!alive) return@runOnUiThread
                duration.doubleValue = value
                if (value > 0) android.util.Log.i("webtor-player", "length on screen: $value s")
                if (value > 0) { reportLength(); releaseLength(play = true) }
            }
            "demuxer-cache-time" -> cached.doubleValue = value
        }
    }
    override fun eventProperty(property: String, value: Boolean) {
        if (!alive) return
        when (property) {
            "pause" -> { paused.value = value; runOnUiThread { updatePip() } }
            "paused-for-cache" -> { cacheWait = value; updateBuffering() }
            "seeking" -> { seekWait = value; updateBuffering() }
            "idle-active" -> if (value && firstFileLoaded) runOnUiThread { if (alive) close() }
            "eof-reached" -> runOnUiThread {
                if (!alive) return@runOnUiThread
                if (value) onEpisodeEnd() else { endHandled = false; nextCard.value = null }
            }
        }
    }
    override fun eventProperty(property: String, value: String) {}
    override fun event(eventId: Int) {
        if (!alive) return
        when (eventId) {
            MPVLib.MPV_EVENT_FILE_LOADED -> {
                // Nothing here may wait on mpv: this thread also delivers the duration,
                // and mpv can be busy for many seconds fetching the first frame. Calls
                // made here used to hold the end time back until playback had begun.
                loading = false; updateBuffering()
                mpvSend {
                    // mpv only reports duration when it changes, so an episode with the same
                    // length as the last one may send nothing: read it once here as well.
                    val known = MPVLib.getPropertyDouble("duration")
                    openedWithoutLength = known == null || known <= 0
                    if (known != null && known > 0) runOnUiThread {
                        if (!alive) return@runOnUiThread
                        duration.doubleValue = known
                        reportLength()
                        releaseLength(play = true)
                    }
                }
                firstFileLoaded = true
                // Each episode sets its own start (switchTo); none carries over.
                mpvSend { MPVLib.setPropertyString("start", "none") }
                applyTrackPrefs(listOf("audio", "sub"))
                addSubtitles(subtitlesFor(playlistPos.intValue))
            }
            MPVLib.MPV_EVENT_START_FILE -> {
                // Paused before mpv gets far with the file: the next episode must not
                // play a moment before its length is known. Queued, never waited on here.
                mpvSend { MPVLib.setPropertyBoolean("pause", true) }
                openedWithoutLength = false
                loading = true; seekPending.value = false; updateBuffering()
                runOnUiThread { holdForLength() }
            }
            MPVLib.MPV_EVENT_PLAYBACK_RESTART -> { loading = false; seekWait = false; seekPending.value = false; updateBuffering() }
            MPVLib.MPV_EVENT_VIDEO_RECONFIG -> runOnUiThread { updatePip(); if (inPip.value) pipBuffer(true) }
        }
    }

    data class Track(val id: String, val name: String, val detail: String, val selected: Boolean)

    /** Audio or subtitle tracks from mpv's track-list; subtitles get an "Off" row. */
    fun tracks(type: String): List<Track> = mpv { readTracks(type) } ?: emptyList()

    private fun readTracks(type: String): List<Track> {
        val count = MPVLib.getPropertyInt("track-list/count") ?: 0
        val list = (0 until count).mapNotNull { i ->
            if (MPVLib.getPropertyString("track-list/$i/type") != type) return@mapNotNull null
            val id = MPVLib.getPropertyInt("track-list/$i/id") ?: return@mapNotNull null
            val title = MPVLib.getPropertyString("track-list/$i/title")
            val lang = MPVLib.getPropertyString("track-list/$i/lang")
            val codec = MPVLib.getPropertyString("track-list/$i/codec")
            val channels = if (type == "audio") MPVLib.getPropertyInt("track-list/$i/demux-channel-count")?.let { c ->
                when (c) { 1 -> "mono"; 2 -> "stereo"; 6 -> "5.1"; 8 -> "7.1"; else -> "$c ch" } } else null
            val external = MPVLib.getPropertyBoolean("track-list/$i/external") == true
            val name = listOfNotNull(
                lang?.let { java.util.Locale.forLanguageTag(it).displayLanguage.ifBlank { it } },
                title?.takeIf { it.isNotBlank() && it != lang },
            ).joinToString(" · ").ifBlank { "Track $id" }
            val detail = listOfNotNull(codec?.uppercase(), channels, if (external) "file" else null).joinToString(" · ")
            Track(id.toString(), name, detail, MPVLib.getPropertyBoolean("track-list/$i/selected") == true)
        }
        return if (type == "sub") listOf(Track("no", "Off", "", list.none { it.selected })) + list else list
    }

    fun selectTrack(type: String, id: String) {
        mpvSend {
            MPVLib.setPropertyString(if (type == "audio") "aid" else "sid", id)
            // Kept for the next episode and, per torrent, for the next time it plays.
            val pref = if (id == "no") TrackPrefs.Pref(true, null, null, false)
                else trackInfos(type).firstOrNull { it.id.toString() == id }?.let { TrackPrefs.Pref(false, it.lang, it.title, it.external) }
            if (pref != null) {
                synchronized(sessionPrefs) { sessionPrefs[type] = pref }
                torrentKey?.let { TrackPrefs.save(this, it, type, pref) }
            }
        }
    }

    private class TrackInfo(val id: Int, val lang: String?, val title: String?, val external: Boolean)

    /** Only on mpv's command thread or under [mpv]. */
    private fun trackInfos(type: String): List<TrackInfo> {
        val count = MPVLib.getPropertyInt("track-list/count") ?: 0
        return (0 until count).mapNotNull { i ->
            if (MPVLib.getPropertyString("track-list/$i/type") != type) return@mapNotNull null
            TrackInfo(
                MPVLib.getPropertyInt("track-list/$i/id") ?: return@mapNotNull null,
                MPVLib.getPropertyString("track-list/$i/lang")?.ifBlank { null },
                MPVLib.getPropertyString("track-list/$i/title")?.ifBlank { null },
                MPVLib.getPropertyBoolean("track-list/$i/external") == true,
            )
        }
    }

    /** Choices made in this player; used on the command thread only. */
    private val sessionPrefs = HashMap<String, TrackPrefs.Pref>()

    private fun trackPref(type: String): TrackPrefs.Pref? =
        torrentKey?.let { TrackPrefs.load(this, it, type) } ?: synchronized(sessionPrefs) { sessionPrefs[type] }

    /**
     * The kept audio and subtitle choice on this episode: the same track by language and
     * name, else the same language, else mpv's default. Subtitles Off stays off.
     */
    private fun applyTrackPrefs(types: List<String>) = mpvSend {
        types.forEach { type ->
            val p = trackPref(type) ?: return@forEach
            val prop = if (type == "audio") "aid" else "sid"
            if (p.off) { if (type == "sub") MPVLib.setPropertyString("sid", "no"); return@forEach }
            val list = trackInfos(type)
            val match = list.firstOrNull { it.lang == p.lang && it.title == p.title && it.external == p.external }
                ?: (if (p.external) list.firstOrNull { it.external && (p.lang == null || it.lang == p.lang) } else null)
                ?: p.lang?.let { l -> list.firstOrNull { it.lang == l } }
            match?.let { MPVLib.setPropertyString(prop, it.id.toString()) }
        }
    }

    fun togglePause() {
        if (lengthWait.value) {
            // No playing before the end time is known.
            if (openedWithoutLength) releaseLength(play = true)
            return
        }
        mpvSend { MPVLib.command(arrayOf("cycle", "pause")) }
    }
    fun seekBy(seconds: Int) { markSeek(); mpvSend { MPVLib.command(arrayOf("seek", seconds.toString(), "relative+keyframes")) } }
    fun seekTo(seconds: Double) { markSeek(); mpvSend { MPVLib.command(arrayOf("seek", "%.3f".format(java.util.Locale.ROOT, seconds), "absolute+keyframes")) } }
    fun cycle(what: String) { mpvSend { MPVLib.command(arrayOf("osd-msg", "cycle", what)) } }
    fun next() = switchTo(playlistPos.intValue + 1)
    fun previous() = switchTo(playlistPos.intValue - 1)

    /** Library torrent and file behind a playlist item (engine stream or app content URI). */
    private fun fileOf(item: Item): Pair<DownloadEntry, SavedFile>? {
        val library = (application as WebtorApp).session.ui.value.library
        val (entry, index) = torrentOf(item.uri)?.let { (id, i) -> (library.find { it.engineId == id } ?: return null) to i }
            ?: item.uri.takeIf { it.scheme == "content" && it.pathSegments.size >= 2 }?.let { u ->
                (library.find { it.key == u.pathSegments[0] } ?: return null) to (u.pathSegments[1].toIntOrNull() ?: return null)
            }
            ?: return null
        return entry to (entry.files.find { it.index == index } ?: return null)
    }

    /** Sidecar subtitles of episode [at]: the picked one's came with the intent, others are looked up. */
    private fun subtitlesFor(at: Int): List<Uri> {
        if (at == startIndex) return subtitles.toList()
        val (entry, file) = items.getOrNull(at)?.let(::fileOf) ?: return emptyList()
        return runCatching { playbackFiles(entry, file).subtitles.map { PlaybackProvider.uriFor(entry, it) } }.getOrDefault(emptyList())
    }

    /** Keep where the current episode is, as the library bookmark (no Play next from here: the player does that). */
    private fun savePosition(atEnd: Boolean = false) {
        val item = items.getOrNull(playlistPos.intValue) ?: return
        val dur = duration.doubleValue
        if (dur <= 0) return
        val (entry, file) = fileOf(item) ?: return
        val posMs = ((if (atEnd) dur else position.doubleValue) * 1000).toLong()
        (application as WebtorApp).session.recordPlaybackPosition(
            PlaybackProvider.uriFor(entry, file).toString(), posMs, (dur * 1000).toLong(), autoNext = false)
    }

    /** Move to episode [at] in this same player: save the one left, open the new one at its own bookmark. */
    private fun switchTo(at: Int) {
        if (at !in items.indices || finishing || !alive) return
        nextCard.value = null
        if (at == playlistPos.intValue) return
        savePosition()
        endHandled = false
        playlistPos.intValue = at
        val item = items[at]
        title.value = item.name
        position.doubleValue = 0.0; cached.doubleValue = 0.0
        duration.doubleValue = 0.0 // the old episode's length must not show under the new title
        spans.value = emptyList(); fileDone.value = null; currentFile = null
        val target = fileOf(item)
        val start = target?.second?.takeIf { it.playbackPositionMs > 1000 }?.playbackPositionMs?.div(1000.0)
        target?.let { (entry, file) -> (application as WebtorApp).session.playerSwitched(entry.key, file.index) }
        loading = true; updateBuffering()
        mpvSend {
            MPVLib.setPropertyString("start", start?.let { "%.3f".format(java.util.Locale.ROOT, it) } ?: "none")
            MPVLib.command(arrayOf("loadfile", mpvPath(item.uri), "replace"))
        }
    }

    /** End of an episode: keep it as watched, then the Up next card, or close after the last one. */
    private fun onEpisodeEnd() {
        if (finishing || endHandled || duration.doubleValue <= 0) return
        endHandled = true
        savePosition(atEnd = true)
        val next = playlistPos.intValue + 1
        if (next >= items.size) { close(); return }
        nextCard.value = next
    }

    private fun close() {
        if (finishing) return
        finishing = true
        val item = items.getOrNull(playlistPos.intValue) ?: items.firstOrNull()
        val result = Intent().apply {
            item?.let { data = it.uri }
            putExtra("position", (position.doubleValue * 1000).toLong())
            putExtra("duration", (duration.doubleValue * 1000).toLong())
            putExtra("webtor_builtin", true)
        }
        setResult(Activity.RESULT_OK, result)
        finish()
    }

    // ---- Picture in picture ----

    private val inPip = mutableStateOf(false)
    private val pipAction = "webtor.app.PIP_PLAY_PAUSE"
    private val pipReceiver = object : android.content.BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) { togglePause() }
    }

    private var videoView: SurfaceView? = null

    /**
     * Where the picture is on screen, in [ratio]'s shape. With it the system zooms the
     * real video into and out of the small window; without it Android 12+ shows the
     * app icon over a plain colour during the switch.
     */
    private fun videoRect(ratio: Double): android.graphics.Rect? {
        val view = videoView ?: return null
        val r = android.graphics.Rect()
        if (!view.getGlobalVisibleRect(r) || r.width() <= 0 || r.height() <= 0) return null
        val w = r.width().toDouble(); val h = r.height().toDouble()
        return if (w / h > ratio) {
            val fit = (h * ratio).toInt(); val x = r.left + (r.width() - fit) / 2
            android.graphics.Rect(x, r.top, x + fit, r.bottom)
        } else {
            val fit = (w / ratio).toInt(); val y = r.top + (r.height() - fit) / 2
            android.graphics.Rect(r.left, y, r.right, y + fit)
        }
    }

    private fun pipParams(): android.app.PictureInPictureParams? {
        if (Build.VERSION.SDK_INT < 26 || !alive || finishing || isFinishing || isDestroyed) return null
        val aspect = videoAspect.takeIf { it > 0 } ?: (16.0 / 9)
        val clamped = aspect.coerceIn(1 / 2.39, 2.39)
        val ratio = android.util.Rational((clamped * 1000).toInt(), 1000)
        val playing = !paused.value
        val action = android.app.RemoteAction(
            android.graphics.drawable.Icon.createWithResource(this,
                if (playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play),
            if (playing) "Pause" else "Play", if (playing) "Pause" else "Play",
            android.app.PendingIntent.getBroadcast(this, 1, Intent(pipAction).setPackage(packageName),
                android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT),
        )
        return android.app.PictureInPictureParams.Builder().setAspectRatio(ratio).setActions(listOf(action)).apply {
            videoRect(ratio.toDouble())?.let { setSourceRectHint(it) }
            // Seamless resize on (Android's advice for video): off made the system lay a
            // still overlay on the window for ~0.6 s after every PiP resize.
            if (Build.VERSION.SDK_INT >= 31) setAutoEnterEnabled(!finishing).setSeamlessResizeEnabled(true)
        }.build()
    }

    fun enterPip() {
        if (Build.VERSION.SDK_INT < 26) return
        if (!packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_PICTURE_IN_PICTURE)) return
        pipParams()?.let { runCatching { enterPictureInPictureMode(it) } }
    }

    /**
     * In PiP the video buffer keeps one size, the video's own in the window's
     * shape, and the system scales it with the window. Resizing PiP then never
     * makes mpv reallocate and redraw mid-animation, which flashed a wrong frame.
     * Full screen goes back to the view's size (aspect modes need it).
     */
    // Video shape as mpv reports it: kept here so the UI thread never asks a busy mpv.
    @Volatile private var videoAspect = 0.0
    @Volatile private var videoW = 0
    @Volatile private var videoH = 0

    private fun pipBuffer(on: Boolean) {
        val holder = videoView?.holder ?: return
        if (!on) { holder.setSizeFromLayout(); return }
        val ratio = (videoAspect.takeIf { it > 0 } ?: (16.0 / 9))
            .coerceIn(1 / 2.39, 2.39) // as the PiP window's ratio
        val dw = videoW.takeIf { it > 0 } ?: 1280
        val dh = videoH.takeIf { it > 0 } ?: 720
        // Long side at the video's own resolution, at most 1920 (PiP is small).
        val long = maxOf(dw, dh).coerceIn(640, 1920)
        val (w, h) = if (ratio >= 1) long to (long / ratio).toInt() else (long * ratio).toInt() to long
        holder.setFixedSize(w and 1.inv(), h and 1.inv())
    }

    private fun updatePip() {
        if (Build.VERSION.SDK_INT >= 26) pipParams()?.let { runCatching { setPictureInPictureParams(it) } }
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        // Leaving the app: keep the video in a small window (older Android; 12+ auto-enters).
        if (Build.VERSION.SDK_INT in 26..30 && !finishing) enterPip()
    }

    override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: android.content.res.Configuration) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        inPip.value = isInPictureInPictureMode
        pipBuffer(isInPictureInPictureMode)
        // The library shows behind the small window: give it this episode and spot now.
        if (isInPictureInPictureMode) savePosition()
        // The page went into the small window with its video: the app behind it goes back to
        // the library, not to a second copy of the same page. Tapping the card brings it back.
        if (isInPictureInPictureMode && pageMode) {
            val session = (application as WebtorApp).session
            if (session.ui.value.openTorrentKey == detailKey) session.closeFiles()
        }
        // Closed from the small window: stop and report the position.
        if (!isInPictureInPictureMode && lifecycle.currentState == androidx.lifecycle.Lifecycle.State.CREATED) close()
    }

    override fun onPause() {
        super.onPause()
        if (!finishing) savePosition()
    }

    override fun onStop() {
        super.onStop()
        releaseLength(play = false)
        if (!isFinishing) mpvSend { MPVLib.setPropertyBoolean("pause", true) }
    }

    override fun finish() {
        if (!finishing) {
            // System back or idle end: still report where playback stopped.
            finishing = true
            val item = items.getOrNull(playlistPos.intValue) ?: items.firstOrNull()
            setResult(Activity.RESULT_OK, Intent().apply {
                item?.let { data = it.uri }
                putExtra("position", (position.doubleValue * 1000).toLong())
                putExtra("duration", (duration.doubleValue * 1000).toLong())
                putExtra("webtor_builtin", true)
            })
        }
        teardown() // free the player now, not later in onDestroy
        super.finish()
        // No video and no small window left on this torrent: Direct Play stops downloading.
        // A moment later, so a player taking over the same torrent (page <-> full screen) keeps it.
        torrentKey?.let { key ->
            val app = application as WebtorApp
            android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
                val next = owner
                if (next == null || !next.alive || next.finishing || next.torrentKey != key) app.session.pauseDirectPlayAfterPlayer(key)
            }, 800)
        }
        if (pageMode) {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }

    override fun onDestroy() {
        if (pageMode) runCatching { orientationListener.disable() }
        runCatching { unregisterReceiver(pipReceiver) }
        teardown()
        super.onDestroy()
    }

    // ---- Torrent info for the playing file (http stream only) ----

    /** One stretch of the file with the share of verified pieces in it, positions 0..1. */
    data class Span(val from: Float, val to: Float, val done: Float)

    private val spans = mutableStateOf<List<Span>>(emptyList())
    @Volatile private var currentFile: Triple<String, Int, Long>? = null
    private var prefetchJob: kotlinx.coroutines.Job? = null

    /**
     * While the user is still dragging, ask the engine for the bytes around the
     * spot they are aiming at, so the seek starts on data already arriving.
     * Time maps to bytes by the file's average bitrate.
     */
    private fun prefetchFor(targetSeconds: Double, durationSeconds: Double) {
        val (id, index, length) = currentFile ?: return
        if (durationSeconds <= 0) return
        if (readyAt((targetSeconds / durationSeconds).toFloat()) == true) return
        prefetchJob?.cancel()
        prefetchJob = lifecycleScope.launch(Dispatchers.IO) {
            delay(150) // only the spot the finger rests on
            val estimate = (length * (targetSeconds / durationSeconds)).toLong()
            runCatching { (application as WebtorApp).engineClient.prefetch(id, index, estimate - (2L shl 20), 10L shl 20) }
        }
    }
    private val swarm = mutableStateOf<String?>(null)
    private val fileDone = mutableStateOf<Int?>(null)

    /** Engine id and file index from http://127.0.0.1:port/torrent/<id>/file/<n>. */
    private fun torrentOf(uri: Uri): Pair<String, Int>? {
        if (uri.scheme != "http") return null
        val seg = uri.pathSegments
        val t = seg.indexOf("torrent")
        if (t < 0 || seg.size < t + 4 || seg[t + 2] != "file") return null
        return seg[t + 1] to (seg[t + 3].toIntOrNull() ?: return null)
    }

    private val directPlay = mutableStateOf(false)

    private fun libraryEntry(id: String): DownloadEntry? =
        (application as WebtorApp).session.ui.value.library.find { it.engineId == id }

    private fun refreshTorrentInfo() {
        val item = items.getOrNull(playlistPos.intValue) ?: return
        val (id, fileIndex) = torrentOf(item.uri) ?: run { spans.value = emptyList(); swarm.value = null; fileDone.value = null; return }
        val client = (application as WebtorApp).engineClient
        val status = runCatching { client.torrent(id) }.getOrNull() ?: return
        directPlay.value = libraryEntry(id)?.directPlay == true
        swarm.value = "${status.numPeers} peers · ${formatSpeed(status.downloadSpeed)}"
        val files = status.files.sortedBy { it.index }
        val file = files.find { it.index == fileIndex } ?: return
        if (file.length <= 0) return
        fileDone.value = (file.progress * 100).toInt().coerceIn(0, 100)
        if (file.progress >= 1.0) { spans.value = listOf(Span(0f, 1f, 1f)); return }
        currentFile = Triple(id, fileIndex, file.length)
        val offset = files.takeWhile { it.index != fileIndex }.sumOf { it.length }
        val pieces = runCatching { client.pieces(id, 4096) }.getOrNull() ?: return
        val pl = pieces.pieceLength.takeIf { it > 0 } ?: return
        val end = offset + file.length
        spans.value = pieces.buckets.mapNotNull { b ->
            val a = maxOf(b.start * pl, offset); val z = minOf((b.end + 1L) * pl, end)
            if (z <= a || b.total <= 0) null
            else Span(((a - offset).toDouble() / file.length).toFloat(), ((z - offset).toDouble() / file.length).toFloat(),
                b.verified.toFloat() / b.total)
        }
    }

    /** True when the spot at [fraction] of the file is already downloaded. */
    private fun readyAt(fraction: Float): Boolean? {
        val list = spans.value.takeIf { it.isNotEmpty() } ?: return null
        return list.firstOrNull { fraction >= it.from && fraction <= it.to }?.let { it.done >= 0.99f }
    }

    // ---- Volume, brightness, speed ----

    private val audio by lazy { getSystemService(Context.AUDIO_SERVICE) as android.media.AudioManager }

    // Brightness as in mpvEx: the player's own level for its window, never the phone's setting.
    // It starts from the phone's brightness (0-255 read as 0-1) and, with "Remember brightness"
    // on, every video opens at the level last set here.
    private fun currentBrightness(): Float {
        val w = window.attributes.screenBrightness
        if (w >= 0) return w
        return runCatching {
            android.provider.Settings.System.getInt(contentResolver, android.provider.Settings.System.SCREEN_BRIGHTNESS) / 255f
        }.getOrDefault(0f)
    }

    private fun setBrightness(v: Float) {
        val level = v.coerceIn(0f, 1f)
        window.attributes = window.attributes.apply { screenBrightness = level }
        if (BuiltInPlayer.rememberBrightness(this)) BuiltInPlayer.setSavedBrightness(this, level)
    }

    private fun volumeFraction(): Float {
        val max = audio.getStreamMaxVolume(android.media.AudioManager.STREAM_MUSIC).coerceAtLeast(1)
        return audio.getStreamVolume(android.media.AudioManager.STREAM_MUSIC).toFloat() / max
    }

    private fun setVolume(v: Float) {
        val max = audio.getStreamMaxVolume(android.media.AudioManager.STREAM_MUSIC)
        audio.setStreamVolume(android.media.AudioManager.STREAM_MUSIC, (v.coerceIn(0f, 1f) * max).roundToInt(), 0)
    }

    private fun setSpeed(x: Double) { mpvSend { MPVLib.setPropertyDouble("speed", x) } }

    private val aspect by lazy { mutableStateOf(PlayerAspect.load(this)) }
    private val rotationLocked = mutableStateOf(false)

    /** Fit keeps the whole picture, Crop fills the screen, Stretch ignores the video's shape. */
    private fun applyAspect(mode: PlayerAspect) = mpvSend {
        MPVLib.setPropertyString("keepaspect", if (mode == PlayerAspect.STRETCH) "no" else "yes")
        MPVLib.setPropertyString("panscan", if (mode == PlayerAspect.CROP) "1.0" else "0.0")
        MPVLib.setPropertyString("video-aspect-override", when (mode) {
            PlayerAspect.R16_9 -> "16:9"
            PlayerAspect.R4_3 -> "4:3"
            else -> "-1"
        })
    }

    private fun cycleAspect(): PlayerAspect = setAspect(aspect.value.next())

    /** Bumped on every user change, so the big center label shows once. */
    private val aspectFlash = mutableIntStateOf(0)

    private fun setAspect(mode: PlayerAspect): PlayerAspect {
        aspect.value = mode
        aspectFlash.intValue++
        PlayerAspect.save(this, mode)
        applyAspect(mode)
        return mode
    }

    /** Lock keeps the side the screen is on now; unlocked follows the sensor between both landscape sides. */
    private fun toggleRotationLock(): Boolean {
        val lock = !rotationLocked.value
        rotationLocked.value = lock
        unlockWhen = 0
        requestedOrientation = if (lock) ActivityInfo.SCREEN_ORIENTATION_LOCKED
            else if (pageMode) ActivityInfo.SCREEN_ORIENTATION_USER else ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        return lock
    }

    private enum class Drag { NONE, VOLUME, BRIGHTNESS, SEEK, SPEED, IGNORE }

    @Composable
    private fun PlayerScreen(surface: (Context) -> SurfaceView, onBack: () -> Unit, compact: Boolean = false) {
        var controls by remember { mutableStateOf(true) }
        var touchTick by remember { mutableIntStateOf(0) }
        var sliderDragging by remember { mutableStateOf(false) }
        var sliderValue by remember { mutableFloatStateOf(0f) }
        var popup by remember { mutableStateOf<String?>(null) }
        var popupSub by remember { mutableStateOf<String?>(null) }
        var popupLevel by remember { mutableStateOf<Float?>(null) }
        var fast by remember { mutableStateOf(false) }
        // Hold speed as in mpvEx: starts at 2x, slide to change, swipe up while holding to keep it.
        var holdSpeed by remember { mutableStateOf(2.0) }
        var speedLocked by remember { mutableStateOf(false) }
        val view = androidx.compose.ui.platform.LocalView.current
        var trackSheet by remember { mutableStateOf<String?>(null) }
        // Double-tap skip streak, like MX Player: side (-1 / +1) and seconds so far.
        var skipSide by remember { mutableIntStateOf(0) }
        var skipTotal by remember { mutableIntStateOf(0) }
        var skipTick by remember { mutableIntStateOf(0) }
        var scrubOrigin by remember { mutableStateOf<Double?>(null) }
        // After release, hold the bar at the target until mpv gets there (no jump back).
        var pendingSeek by remember { mutableStateOf<Double?>(null) }
        var lastScrubAt by remember { mutableStateOf(0L) }
        val pos by position
        val dur by duration
        val isPaused by paused
        val isBuffering by buffering
        val seekingNow by seekPending
        // Never spin forever if mpv drops a seek.
        LaunchedEffect(seekingNow) { if (seekingNow) { delay(20_000); seekPending.value = false } }

        BackHandler(onBack = onBack)
        LaunchedEffect(controls, touchTick, isPaused, sliderDragging, trackSheet) {
            if (controls && !isPaused && !sliderDragging && trackSheet == null) { delay(3500); controls = false }
        }
        LaunchedEffect(pendingSeek) { if (pendingSeek != null) { delay(4000); pendingSeek = null } }
        LaunchedEffect(pos, isBuffering) {
            pendingSeek?.let { t -> if (kotlin.math.abs(pos - t) < 3 && !seekWait) pendingSeek = null }
        }
        LaunchedEffect(skipTick) {
            if (skipSide != 0) { delay(800); skipSide = 0; skipTotal = 0 }
        }
        LaunchedEffect(Unit) {
            while (true) {
                withContext(Dispatchers.IO) { refreshTorrentInfo() }
                delay(1000)
            }
        }

        /**
         * While dragging: show where it lands and move the picture like a normal
         * player when that spot is already downloaded or buffered. Spots not yet
         * downloaded are only prefetched, then seeked once on release.
         */
        fun seekPopup(target: Double) {
            val origin = scrubOrigin ?: pos.also { scrubOrigin = it }
            val delta = target - origin
            popup = clock(target) + "  [" + (if (delta >= 0) "+" else "-") + clock(kotlin.math.abs(delta)) + "]"
            val ready = if (dur > 0) readyAt((target / dur).toFloat()) else null
            val inBuffer = target >= pos - 5 && target <= cached.doubleValue
            popupSub = if (ready == false && !inBuffer) "will buffer" else null
            popupLevel = null
            if (ready == true || inBuffer) {
                val now = System.currentTimeMillis()
                if (now - lastScrubAt > 250 && !seekWait) { lastScrubAt = now; seekTo(target) }
            } else prefetchFor(target, dur)
        }
        fun endScrub() { scrubOrigin = null; popup = null; popupSub = null }

        // Loading: waiting for data while playing, for the length, or for a seek (paused or not).
        val spinning = (isBuffering && (!isPaused || lengthWait.value)) || (seekingNow && scrubOrigin == null && !sliderDragging)
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            AndroidView(factory = surface, modifier = Modifier.fillMaxSize())

            if (inPip.value) return@Box
            // Gesture layer: tap, double tap, hold for 2x, swipes.
            Box(
                Modifier.fillMaxSize()
                    .pointerInput(Unit) {
                        var held = false
                        var streakPress = false
                        var edgePress = false
                        fun sideOf(x: Float) = when { x < size.width / 3f -> -1; x > size.width * 2f / 3f -> 1; else -> 0 }
                        detectTapGestures(
                            onPress = press@{ o ->
                                held = false
                                edgePress = edgeTouch
                                if (edgePress) { streakPress = false; tryAwaitRelease(); return@press }
                                // During a skip streak every tap on the same side adds 10 s at once.
                                val side = sideOf(o.x)
                                streakPress = side != 0 && side == skipSide
                                if (streakPress) { seekBy(10 * side); skipTotal += 10; skipTick++ }
                                coroutineScope {
                                    val hold = launch {
                                        delay(450)
                                        held = true; speedLocked = false; holdSpeed = 2.0; fast = true; setSpeed(2.0)
                                    }
                                    tryAwaitRelease()
                                    hold.cancel()
                                    if (held) { if (!speedLocked) setSpeed(1.0); fast = false }
                                }
                            },
                            onTap = { if (!held && !streakPress && !edgePress) { controls = !controls; touchTick++ } },
                            onDoubleTap = { o ->
                                if (!streakPress && !edgePress) {
                                    val side = sideOf(o.x)
                                    if (side == 0) togglePause()
                                    else { seekBy(10 * side); skipSide = side; skipTotal = 10; skipTick++ }
                                }
                            },
                        )
                    }
                    .pointerInput(compact) {
                        // Small player on the page: swipe up goes full screen, swipe down to PiP (as on YouTube);
                        // no brightness, volume or drag-seek there, the page scrolls instead.
                        // Swipes as in mpvEx: measured from where the finger went down.
                        // Clearly sideways (over 30 px and twice the vertical move) seeks
                        // 0.05 s per pixel with the video paused, and resumes on release.
                        // Up/down: left half brightness, right half volume.
                        awaitEachGesture {
                            val down = awaitFirstDown(requireUnconsumed = false)
                            if (edgeTouch) return@awaitEachGesture // belongs to the system
                            val start = down.position
                            val startAt = System.currentTimeMillis()
                            var mode = Drag.NONE
                            var speedX = 0f
                            var speedY = 0f
                            var level = 0f
                            var origin = 0.0
                            var target = 0.0
                            var wasPaused = false
                            fun finish(apply: Boolean) {
                                if (mode == Drag.SEEK) {
                                    if (apply) { pendingSeek = target; seekTo(target) }
                                    if (!wasPaused) mpvSend { MPVLib.setPropertyBoolean("pause", false) }
                                }
                                if (mode != Drag.NONE) { endScrub(); popupLevel = null }
                            }
                            while (true) {
                                val event = awaitPointerEvent()
                                if (event.changes.count { it.pressed } > 1) { finish(false); break }
                                val change = event.changes.firstOrNull { it.id == down.id } ?: break
                                if (!change.pressed) { finish(true); break }
                                val dx = change.position.x - start.x
                                val dy = change.position.y - start.y
                                val ax = kotlin.math.abs(dx); val ay = kotlin.math.abs(dy)
                                if (mode == Drag.NONE) {
                                    mode = when {
                                        fast -> Drag.SPEED
                                        trackSheet != null -> if (ax > 30f || ay > 30f) Drag.IGNORE else Drag.NONE
                                        compact -> if (ax > 30f || ay > 30f) {
                                            if (dy < -30f && ay > ax) setFullscreen(true)
                                            // Swipe down: keep playing in the small window, the library behind it.
                                            else if (dy > 30f && ay > ax) { controls = false; enterPip() }
                                            Drag.IGNORE
                                        } else Drag.NONE
                                        ax > 30f && ax > ay * 2f && System.currentTimeMillis() - startAt > 100 -> Drag.SEEK
                                        ay > 30f && ay > ax -> if (start.x < size.width / 2f) Drag.BRIGHTNESS else Drag.VOLUME
                                        else -> Drag.NONE
                                    }
                                    if (mode == Drag.SPEED) { speedX = change.position.x; speedY = change.position.y }
                                    when (mode) {
                                        Drag.VOLUME -> level = volumeFraction()
                                        Drag.BRIGHTNESS -> level = currentBrightness()
                                        Drag.SEEK -> {
                                            origin = pos; target = pos
                                            wasPaused = paused.value
                                            if (!wasPaused) mpvSend { MPVLib.setPropertyBoolean("pause", true) }
                                            controls = true; touchTick++ // show the bar while swiping
                                        }
                                        else -> {}
                                    }
                                }
                                when (mode) {
                                    Drag.VOLUME -> {
                                        val v = (level - dy / (size.height * 0.8f)).coerceIn(0f, 1f)
                                        setVolume(v)
                                        popup = "Volume ${(v * 100).roundToInt()}%"; popupSub = null; popupLevel = v
                                    }
                                    Drag.BRIGHTNESS -> {
                                        val v = (level - dy / (size.height * 0.8f)).coerceIn(0f, 1f)
                                        setBrightness(v)
                                        popup = "Brightness ${(v * 100).roundToInt()}%"; popupSub = null; popupLevel = v
                                    }
                                    Drag.SEEK -> {
                                        val next = (origin + dx * 0.05).coerceIn(0.0, if (dur > 0) dur else Double.MAX_VALUE)
                                        if (next.toInt() != target.toInt()) {
                                            target = next
                                            seekPopup(target)
                                            pendingSeek = target // bar follows the finger
                                        }
                                        touchTick++
                                    }
                                    Drag.SPEED -> if (fast) {
                                        // Right is faster, left is slower: about a third of the screen spans all steps.
                                        val step = ((change.position.x - speedX) / size.width * (speeds.size - 1) * 3.5f).toInt()
                                        val next = speeds[(speeds.indexOf(2.0) + step).coerceIn(0, speeds.size - 1)]
                                        if (next != holdSpeed) {
                                            holdSpeed = next; setSpeed(next)
                                            view.performHapticFeedback(android.view.HapticFeedbackConstants.CLOCK_TICK)
                                        }
                                        // Swipe up while holding keeps this speed after letting go.
                                        if (!speedLocked && speedY - change.position.y > 60.dp.toPx()) {
                                            speedLocked = true
                                            view.performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
                                        }
                                    }
                                    Drag.NONE, Drag.IGNORE -> {}
                                }
                                // Once a swipe is decided, taps, double taps and hold-for-2x stand down.
                                // Speed sliding leaves the touch to the hold, which resets on release.
                                if (mode != Drag.NONE && mode != Drag.SPEED) change.consume()
                            }
                        }
                    },
            )

            // Spinner only when controls are hidden; with controls, the play button shows it.
            if (spinning && !controls) {
                Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator(Modifier.size(44.dp), color = Color.White, strokeWidth = 3.dp)
                    swarm.value?.let {
                        Spacer(Modifier.height(12.dp))
                        Text(it, color = Color.White, fontSize = 13.sp,
                            modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(Color.Black.copy(alpha = 0.5f)).padding(horizontal = 10.dp, vertical = 4.dp))
                    }
                }
            }

            AnimatedVisibility(controls && nextCard.value == null, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
                Box(Modifier.fillMaxSize()) {
                    val iconSize = if (compact) 24.dp else 27.dp
                    // Shade only top and bottom so the picture stays clear.
                    Box(Modifier.fillMaxWidth().height(if (compact) 64.dp else 110.dp).align(Alignment.TopCenter)
                        .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.7f), Color.Transparent))))
                    Box(Modifier.fillMaxWidth().height(if (compact) 80.dp else 170.dp).align(Alignment.BottomCenter)
                        .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.8f)))))

                    // Top: back and title only.
                    Row(Modifier.fillMaxWidth().padding(horizontal = if (compact) 4.dp else 24.dp, vertical = if (compact) 0.dp else 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = onBack) {
                            if (compact || pageMode) Icon(Icons.Filled.KeyboardArrowDown, if (compact) "Back" else "Exit full screen",
                                tint = Color.White, modifier = Modifier.size(30.dp))
                            else Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White, modifier = Modifier.size(iconSize))
                        }
                        Column(Modifier.weight(1f).padding(start = 4.dp)) {
                            if (!compact) Text(episodeTitle(title.value), color = Color.White, style = MaterialTheme.typography.titleMedium,
                                maxLines = 1, overflow = TextOverflow.Ellipsis)
                            // Downloads show progress; Direct Play keeps video in RAM, so a percentage means nothing there.
                            val lead = if (directPlay.value) "Direct Play · streaming from RAM" else fileDone.value?.let { "$it% downloaded" }
                            val info = listOfNotNull(lead, swarm.value).joinToString("  ·  ")
                            if (info.isNotEmpty()) Text(info, color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelMedium,
                                maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                        IconButton(onClick = { trackSheet = "audio"; controls = false }) {
                            Icon(Icons.Filled.Audiotrack, "Audio", tint = Color.White, modifier = Modifier.size(iconSize))
                        }
                        IconButton(onClick = { trackSheet = "sub"; controls = false }) {
                            Icon(Icons.Filled.Subtitles, "Subtitles", tint = Color.White, modifier = Modifier.size(iconSize))
                        }
                        // Tap: next aspect. Hold: pick from a list, like audio and subtitles.
                        val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current
                        if (!compact) Box(
                            Modifier.size(48.dp).clip(androidx.compose.foundation.shape.CircleShape)
                                .pointerInput(Unit) {
                                    detectTapGestures(
                                        onTap = { cycleAspect(); touchTick++ },
                                        onLongPress = {
                                            haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.LongPress)
                                            trackSheet = "aspect"; controls = false
                                        },
                                    )
                                },
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(Icons.Filled.AspectRatio, "Aspect ratio: ${aspect.value.label}. Hold for the list.",
                                tint = Color.White, modifier = Modifier.size(iconSize))
                        }
                        if (!compact) IconButton(onClick = { toggleRotationLock(); touchTick++ }) {
                            Icon(
                                if (rotationLocked.value) Icons.Filled.ScreenLockRotation else Icons.Filled.ScreenRotation,
                                if (rotationLocked.value) "Unlock rotation" else "Lock rotation",
                                tint = Color.White, modifier = Modifier.size(iconSize),
                            )
                        }
                    }

                    // Center: rewind, play/pause, forward.
                    val side = if (compact) 52.dp else 64.dp
                    val sideIcon = if (compact) 32.dp else 40.dp
                    val mid = if (compact) 60.dp else 80.dp
                    val midIcon = if (compact) 38.dp else 50.dp
                    // A series adds Previous and Next episode on both sides.
                    val episodes = items.size > 1
                    val at = playlistPos.intValue
                    val gap = when { compact && episodes -> 14.dp; compact -> 40.dp; episodes -> 40.dp; else -> 56.dp }
                    Row(Modifier.align(Alignment.Center), horizontalArrangement = Arrangement.spacedBy(gap), verticalAlignment = Alignment.CenterVertically) {
                        if (episodes) IconButton(onClick = { previous(); touchTick++ }, enabled = at > 0, modifier = Modifier.size(side)) {
                            Icon(Icons.Filled.SkipPrevious, "Previous episode", tint = Color.White.copy(alpha = if (at > 0) 1f else 0.35f),
                                modifier = Modifier.size(sideIcon))
                        }
                        IconButton(onClick = { seekBy(-10); touchTick++ }, modifier = Modifier.size(side)) {
                            Icon(Icons.Filled.Replay10, "Back 10 seconds", tint = Color.White, modifier = Modifier.size(sideIcon))
                        }
                        androidx.compose.material3.FilledTonalIconButton(
                            onClick = { togglePause(); touchTick++ },
                            modifier = Modifier.size(mid),
                            colors = androidx.compose.material3.IconButtonDefaults.filledTonalIconButtonColors(
                                containerColor = Color.Black.copy(alpha = 0.4f), contentColor = Color.White),
                        ) {
                            when {
                                spinning -> CircularProgressIndicator(Modifier.size(if (compact) 34.dp else 44.dp), color = Color.White, strokeWidth = 3.dp)
                                isPaused -> Icon(Icons.Filled.PlayArrow, "Play", modifier = Modifier.size(midIcon))
                                else -> Icon(Icons.Filled.Pause, "Pause", modifier = Modifier.size(midIcon))
                            }
                        }
                        IconButton(onClick = { seekBy(10); touchTick++ }, modifier = Modifier.size(side)) {
                            Icon(Icons.Filled.Forward10, "Forward 10 seconds", tint = Color.White, modifier = Modifier.size(sideIcon))
                        }
                        if (episodes) IconButton(onClick = { next(); touchTick++ }, enabled = at < items.size - 1, modifier = Modifier.size(side)) {
                            Icon(Icons.Filled.SkipNext, "Next episode", tint = Color.White.copy(alpha = if (at < items.size - 1) 1f else 0.35f),
                                modifier = Modifier.size(sideIcon))
                        }
                    }

                    // Bottom: time, seek bar, then actions.
                    Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().then(
                        if (compact) Modifier.padding(horizontal = 8.dp) else Modifier.padding(start = 32.dp, end = 32.dp, top = 14.dp, bottom = 28.dp))) {
                        val shown = when {
                            sliderDragging -> sliderValue.toDouble()
                            else -> pendingSeek ?: pos
                        }
                        // As on YouTube: "0:03 / 1:34:17" on the left, the screen-size button on the right.
                        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(clock(shown), color = Color.White, style = MaterialTheme.typography.labelLarge)
                            Text(" / " + clock(dur), color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelLarge)
                            val ahead = (cached.doubleValue - pos).coerceAtLeast(0.0)
                            if (ahead >= 1) Text(if (compact) "  ·  buffered ${clock(ahead)}" else "   ·   buffered ${clock(ahead)}",
                                color = Color.White.copy(alpha = 0.55f),
                                style = if (compact) MaterialTheme.typography.labelSmall else MaterialTheme.typography.labelMedium,
                                maxLines = 1)
                            Spacer(Modifier.weight(1f))
                            // Small window, then full screen in/out, side by side.
                            if (Build.VERSION.SDK_INT >= 26) IconButton(onClick = { controls = false; enterPip() },
                                modifier = Modifier.size(if (compact) 40.dp else 44.dp)) {
                                Icon(Icons.Filled.PictureInPictureAlt, "Picture in picture", tint = Color.White,
                                    modifier = Modifier.size(if (compact) 24.dp else 27.dp))
                            }
                            if (compact) IconButton(onClick = { setFullscreen(true); touchTick++ }, modifier = Modifier.size(40.dp)) {
                                Icon(Icons.Filled.Fullscreen, "Full screen", tint = Color.White, modifier = Modifier.size(28.dp))
                            } else if (pageMode) IconButton(onClick = { setFullscreen(false); touchTick++ }, modifier = Modifier.size(44.dp)) {
                                Icon(Icons.Filled.FullscreenExit, "Exit full screen", tint = Color.White, modifier = Modifier.size(30.dp))
                            }
                        }
                        SeekBar(
                            fraction = if (dur > 0) (shown / dur).toFloat().coerceIn(0f, 1f) else 0f,
                            cachedFraction = if (dur > 0) (cached.doubleValue / dur).toFloat().coerceIn(0f, 1f) else 0f,
                            onDrag = { f ->
                                sliderDragging = true; sliderValue = (f * dur).toFloat()
                                seekPopup(sliderValue.toDouble()); touchTick++
                            },
                            onDragEnd = {
                                pendingSeek = sliderValue.toDouble()
                                seekTo(sliderValue.toDouble()); sliderDragging = false; endScrub(); touchTick++
                            },
                        )
                    }
                }
            }

            // Up next: the following episode starts in 5 s (Play next on); Cancel stays on this one.
            nextCard.value?.let { upNext ->
                val auto = remember(upNext) { (application as WebtorApp).session.ui.value.playNext }
                var left by remember(upNext) { mutableIntStateOf(5) }
                // Counts in the small window too; only the card is hidden there.
                LaunchedEffect(upNext) {
                    if (!auto) return@LaunchedEffect
                    while (left > 0) { delay(1000); left-- }
                    switchTo(upNext)
                }
                if (!inPip.value) Column(
                    Modifier.align(Alignment.Center).padding(horizontal = 16.dp)
                        .clip(RoundedCornerShape(16.dp)).background(Color.Black.copy(alpha = 0.75f))
                        .padding(horizontal = if (compact) 14.dp else 20.dp, vertical = if (compact) 8.dp else 14.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(if (auto) "Up next in $left" else "Up next", color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelMedium)
                    Text(episodeTitle(items.getOrNull(upNext)?.name.orEmpty()), color = Color.White,
                        style = if (compact) MaterialTheme.typography.titleSmall else MaterialTheme.typography.titleMedium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Spacer(Modifier.height(if (compact) 4.dp else 10.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        TextButton(onClick = { nextCard.value = null; controls = true }) { Text("Cancel", color = Color.White) }
                        androidx.compose.material3.Button(onClick = { switchTo(upNext) }) {
                            Icon(Icons.Filled.SkipNext, null, modifier = Modifier.size(18.dp))
                            Spacer(Modifier.width(4.dp))
                            Text("Play now")
                        }
                    }
                }
            }

            trackSheet?.let { type ->
                // The small player is too short for the list: show it over the whole window.
                if (compact) androidx.compose.ui.window.Popup(
                    onDismissRequest = { trackSheet = null },
                    properties = androidx.compose.ui.window.PopupProperties(focusable = true),
                ) { TrackSheet(type, onDismiss = { trackSheet = null }) }
                else TrackSheet(type, onDismiss = { trackSheet = null })
            }

            // Double-tap skip: a soft arc on that side with the running total, like MX Player.
            if (skipSide != 0) {
                val left = skipSide < 0
                Box(
                    Modifier.align(if (left) Alignment.CenterStart else Alignment.CenterEnd)
                        .fillMaxHeight().fillMaxWidth(0.36f)
                        .clip(if (left) RoundedCornerShape(topEndPercent = 50, bottomEndPercent = 50)
                              else RoundedCornerShape(topStartPercent = 50, bottomStartPercent = 50))
                        .background(Color.White.copy(alpha = 0.12f)),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Icon(if (left) Icons.Filled.FastRewind else Icons.Filled.FastForward, null,
                            tint = Color.White, modifier = Modifier.size(32.dp))
                        Text("$skipTotal seconds", color = Color.White, style = MaterialTheme.typography.labelLarge)
                    }
                }
            }

            // Small player on the details page: one small line just above the
            // play button, clear of the title bar.
            if (compact) popup?.let { text ->
                Text(
                    text + (popupSub?.let { "  ·  $it" } ?: ""),
                    color = Color.White,
                    style = MaterialTheme.typography.labelSmall,
                    maxLines = 1,
                    modifier = Modifier.align(Alignment.Center).offset(y = (-48).dp)
                        .clip(RoundedCornerShape(50)).background(Color.Black.copy(alpha = 0.6f))
                        .padding(horizontal = 10.dp, vertical = 3.dp),
                )
            }
            // Small pop-up for swipes and seeking.
            if (!compact) popup?.let { text ->
                Column(
                    Modifier.align(Alignment.TopCenter).padding(top = if (compact) 12.dp else 72.dp)
                        .clip(RoundedCornerShape(10.dp)).background(Color.Black.copy(alpha = 0.6f))
                        .padding(horizontal = 14.dp, vertical = 7.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(text, color = Color.White, style = MaterialTheme.typography.titleSmall)
                    popupLevel?.let { lv ->
                        Spacer(Modifier.height(6.dp))
                        Box(Modifier.width(110.dp).height(3.dp).clip(RoundedCornerShape(2.dp)).background(Color.White.copy(alpha = 0.25f))) {
                            Box(Modifier.fillMaxWidth(lv).height(3.dp).background(Color.White))
                        }
                    }
                    popupSub?.let {
                        Text(it, color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelSmall)
                    }
                }
            }

            // Aspect change: show the new mode big in the center for a moment.
            var aspectLabel by remember { mutableStateOf<String?>(null) }
            LaunchedEffect(aspectFlash.intValue) {
                if (aspectFlash.intValue == 0) return@LaunchedEffect
                aspectLabel = aspect.value.label
                kotlinx.coroutines.delay(1200)
                aspectLabel = null
            }
            AnimatedVisibility(aspectLabel != null && !inPip.value, enter = fadeIn(), exit = fadeOut(),
                // Halfway between the top bar and the play button, so it never covers either.
                modifier = Modifier.align(androidx.compose.ui.BiasAlignment(0f, -0.5f))) {
                Text(aspectLabel.orEmpty(), color = Color.White, style = MaterialTheme.typography.headlineMedium,
                    modifier = Modifier.clip(RoundedCornerShape(16.dp)).background(Color.Black.copy(alpha = 0.6f))
                        .padding(horizontal = 24.dp, vertical = 12.dp))
            }

            // Hold speed: one small pill at the top. The hint shows briefly at the start of a hold.
            // Locked speed stays; tap the pill for normal speed.
            var speedHint by remember { mutableStateOf(false) }
            LaunchedEffect(fast) {
                speedHint = fast
                if (fast) { kotlinx.coroutines.delay(1500); speedHint = false }
            }
            if (fast || speedLocked) {
                Column(
                    Modifier.align(Alignment.TopCenter).padding(top = if (compact) 6.dp else 16.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Row(
                        Modifier.clip(RoundedCornerShape(50)).background(Color.Black.copy(alpha = 0.5f))
                            .then(if (speedLocked && !fast) Modifier.clickable { speedLocked = false; holdSpeed = 2.0; setSpeed(1.0) } else Modifier)
                            .padding(horizontal = 10.dp, vertical = 3.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(speedLabel(holdSpeed), color = Color.White, style = MaterialTheme.typography.labelMedium)
                        Spacer(Modifier.width(3.dp))
                        Icon(if (speedLocked) Icons.Filled.Lock else Icons.Filled.FastForward, null, tint = Color.White, modifier = Modifier.size(12.dp))
                        if (speedLocked && !fast) {
                            Spacer(Modifier.width(3.dp))
                            Icon(Icons.Filled.Close, "Normal speed", tint = Color.White.copy(alpha = 0.7f), modifier = Modifier.size(12.dp))
                        }
                    }
                    if (speedHint && !speedLocked) {
                        Text(
                            "← → speed · ↑ keep",
                            color = Color.White.copy(alpha = 0.75f), style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.padding(top = 3.dp).clip(RoundedCornerShape(50))
                                .background(Color.Black.copy(alpha = 0.4f)).padding(horizontal = 8.dp, vertical = 1.dp),
                        )
                    }
                }
            }
        }
    }

    /**
     * Seek bar: dim track, light where the player has it buffered, white for
     * what was played. Download progress is shown as a number below it.
     */
    @Composable
    private fun SeekBar(fraction: Float, cachedFraction: Float, onDrag: (Float) -> Unit, onDragEnd: () -> Unit) {
        var active by remember { mutableStateOf(false) }
        val drag by rememberUpdatedState(onDrag)
        val dragEnd by rememberUpdatedState(onDragEnd)
        Canvas(
            Modifier.fillMaxWidth().height(40.dp)
                .pointerInput(Unit) {
                    // One handler for tap and drag: the finger may leave the thin bar
                    // (up or down) and the seek still follows it and lands on release.
                    awaitEachGesture {
                        val down = awaitFirstDown()
                        down.consume()
                        active = true
                        drag((down.position.x / size.width).coerceIn(0f, 1f))
                        try {
                            while (true) {
                                val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                                if (!change.pressed) { change.consume(); break }
                                if (change.positionChanged()) drag((change.position.x / size.width).coerceIn(0f, 1f))
                                change.consume()
                            }
                        } finally {
                            active = false
                            dragEnd()
                        }
                    }
                },
        ) {
            val h = (if (active) 6.dp else 4.5.dp).toPx()
            val y = size.height / 2 - h / 2
            val r = CornerRadius(h / 2)
            drawRoundRect(Color.White.copy(alpha = 0.22f), Offset(0f, y), Size(size.width, h), r)
            if (cachedFraction > fraction) drawRect(Color.White.copy(alpha = 0.45f),
                Offset(fraction * size.width, y), Size((cachedFraction - fraction) * size.width, h))
            drawRoundRect(Color.White, Offset(0f, y), Size(fraction * size.width, h), r)
            drawCircle(Color.White, radius = (if (active) 10.dp else 8.dp).toPx(), center = Offset(fraction * size.width, size.height / 2))
        }
    }

    /** Material panel to pick an audio or subtitle track. */
    @Composable
    private fun TrackSheet(type: String, onDismiss: () -> Unit) {
        var list by remember(type) {
            mutableStateOf(if (type == "aspect") PlayerAspect.entries.map { Track(it.name, it.label, it.detail, it == aspect.value) } else tracks(type))
        }
        // Floating box in the centre; long lists scroll inside it instead of being cut off.
        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.55f))
            .pointerInput(Unit) { detectTapGestures { onDismiss() } }, contentAlignment = Alignment.Center) {
            androidx.compose.material3.Surface(
                modifier = Modifier.width(360.dp)
                    .heightIn(max = (androidx.compose.ui.platform.LocalConfiguration.current.screenHeightDp * 0.85f).dp)
                    .pointerInput(Unit) { detectTapGestures { } },
                shape = RoundedCornerShape(24.dp),
                tonalElevation = 6.dp,
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
            ) {
                Column(Modifier.padding(vertical = 14.dp)) {
                    Row(Modifier.padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(when (type) { "audio" -> Icons.Filled.Audiotrack; "aspect" -> Icons.Filled.AspectRatio; else -> Icons.Filled.Subtitles }, null,
                            tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.width(12.dp))
                        Text(when (type) { "audio" -> "Audio"; "aspect" -> "Aspect ratio"; else -> "Subtitles" }, style = MaterialTheme.typography.titleMedium,
                            color = MaterialTheme.colorScheme.onSurface, modifier = Modifier.weight(1f))
                        val count = if (type == "sub") list.size - 1 else list.size
                        if (count > 0 && type != "aspect") Text("$count", style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Spacer(Modifier.height(8.dp))
                    if (list.isEmpty() || (type == "sub" && list.size == 1)) {
                        Text(if (type == "audio") "No audio tracks" else "No subtitles in this video",
                            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp))
                    }
                    Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 8.dp)) {
                        list.forEach { t ->
                            val pick = {
                                if (type == "aspect") PlayerAspect.entries.firstOrNull { it.name == t.id }?.let { setAspect(it) }
                                else { selectTrack(type, t.id); list = tracks(type) }
                                onDismiss()
                            }
                            Row(
                                Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp))
                                    .background(if (t.selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
                                    .clickable(onClick = pick).padding(horizontal = 6.dp, vertical = 2.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                androidx.compose.material3.RadioButton(selected = t.selected, onClick = pick, modifier = Modifier.size(36.dp))
                                Column(Modifier.padding(start = 4.dp)) {
                                    Text(t.name, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurface)
                                    if (t.detail.isNotBlank()) Text(t.detail, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                        style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

private val speeds = listOf(0.25, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0)

private fun speedLabel(x: Double) = (if (x % 1.0 == 0.0) x.toInt().toString() else x.toString()) + "×"

private fun clock(seconds: Double): String {
    val s = seconds.toLong().coerceAtLeast(0)
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

/** The few settings the player's theme needs, so it skips every other state change. */
private data class Look(val dark: Boolean, val theme: AppTheme, val reduced: Boolean, val dynamic: Boolean)

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
import androidx.compose.material.icons.filled.FastRewind
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PictureInPictureAlt
import androidx.compose.material.icons.filled.Replay10
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.Subtitles
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
import kotlin.math.roundToInt

/** Picture size in the built-in player, remembered between videos. */
enum class PlayerAspect(val label: String) {
    FIT("Fit"), CROP("Crop"), STRETCH("Stretch"), R16_9("16:9"), R4_3("4:3");

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
        private val lock = Any()
    }

    @Volatile private var alive = false

    /** Release the player once. Detach the picture first so no render thread touches a freed context. */
    private fun teardown() = synchronized(lock) {
        if (!alive) return@synchronized
        alive = false
        runCatching { MPVLib.removeObserver(this) }
        runCatching { MPVLib.setPropertyString("vo", "null") }
        runCatching { MPVLib.setOptionString("force-window", "no") }
        runCatching { MPVLib.detachSurface() }
        runCatching { MPVLib.destroy() }
        if (owner === this) owner = null
    }

    private data class Item(val uri: Uri, val name: String)

    private var items: List<Item> = emptyList()
    private var subtitles: List<Uri> = emptyList()
    private var startSeconds = 0.0
    private var firstFileLoaded = false
    private var finishing = false

    private val title = mutableStateOf("")
    private val position = mutableDoubleStateOf(0.0)
    private val duration = mutableDoubleStateOf(0.0)
    private val cached = mutableDoubleStateOf(0.0)
    private val paused = mutableStateOf(false)
    private val buffering = mutableStateOf(true)
    @Volatile private var loading = true
    @Volatile private var cacheWait = false
    @Volatile private var seekWait = false
    private fun updateBuffering() { buffering.value = loading || cacheWait || seekWait }
    private val playlistPos = mutableIntStateOf(0)

    /** True while a touch that began on a system gesture edge (home pill, status bar) is in progress. */
    private var edgeTouch = false

    /**
     * Swiping up from the home pill or down from the top belongs to the system.
     * Those touches never reach the player, so they cannot seek, change volume
     * or pop up the time overlay.
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
        if (edgeTouch) {
            if (ev.actionMasked == android.view.MotionEvent.ACTION_UP || ev.actionMasked == android.view.MotionEvent.ACTION_CANCEL) edgeTouch = false
            return true
        }
        return super.dispatchTouchEvent(ev)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
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
        startMpv()
        androidx.core.content.ContextCompat.registerReceiver(this, pipReceiver, android.content.IntentFilter(pipAction),
            androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED)

        setContent {
            androidx.compose.material3.MaterialTheme(colorScheme = androidx.compose.material3.darkColorScheme(
                primary = Color.White, onPrimary = Color.Black,
            )) { PlayerScreen(
                surface = { SurfaceView(it).apply { holder.addCallback(surfaceCallback) } },
                onBack = ::close,
            ) }
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
            // Play from the chosen video onward, like "play next".
            val at = all.indexOfFirst { it.uri == data }
            if (at < 0) listOf(Item(data, name)) else all.drop(at)
        }
        subtitles = intent.getParcelableArrayExtra("subs")?.mapNotNull { it as? Uri }.orEmpty()
        startSeconds = (intent.extras?.get("position") as? Number)?.toLong()?.div(1000.0) ?: 0.0
        title.value = items.first().name
    }

    /** mpv opens http directly; content URIs go through a descriptor mpv closes itself. */
    private fun mpvPath(uri: Uri): String = when (uri.scheme) {
        "content" -> try {
            contentResolver.openFileDescriptor(uri, "r")?.let { "fdclose://${it.detachFd()}" } ?: uri.toString()
        } catch (_: Exception) { uri.toString() }
        else -> uri.toString()
    }

    private fun startMpv() {
        synchronized(lock) {
            // A previous player still closing must let go before this one starts.
            owner?.takeIf { it !== this }?.teardown()
            MPVLib.create(applicationContext)
            owner = this
            alive = true
        }
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
            "demuxer-max-bytes" to "64MiB",
            "demuxer-max-back-bytes" to "64MiB",
            "hr-seek" to "no",
            "hr-seek-framedrop" to "yes",
            "cache-pause-initial" to "no",
            "network-timeout" to "0",
            "keep-open" to "no",
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
        MPVLib.init()
        applyAspect(aspect.value)
        MPVLib.addObserver(this)
        listOf(
            "time-pos" to MPVLib.MPV_FORMAT_DOUBLE,
            "duration" to MPVLib.MPV_FORMAT_DOUBLE,
            "demuxer-cache-time" to MPVLib.MPV_FORMAT_DOUBLE,
            "pause" to MPVLib.MPV_FORMAT_FLAG,
            "paused-for-cache" to MPVLib.MPV_FORMAT_FLAG,
            "seeking" to MPVLib.MPV_FORMAT_FLAG,
            "idle-active" to MPVLib.MPV_FORMAT_FLAG,
            "playlist-pos" to MPVLib.MPV_FORMAT_INT64,
        ).forEach { (k, f) -> MPVLib.observeProperty(k, f) }

        if (startSeconds > 1) MPVLib.setPropertyString("start", "%.3f".format(java.util.Locale.ROOT, startSeconds))
    }

    private var filesLoaded = false

    /** Open the files once a surface is attached. Off the main thread, as mpvEx does, so a slow stream never freezes the UI. */
    private fun loadFilesOnce() {
        if (filesLoaded) return
        filesLoaded = true
        val paths = items.map { it.uri }
        lifecycleScope.launch(Dispatchers.Default) {
            paths.forEachIndexed { i, uri ->
                if (!alive) return@launch
                MPVLib.command(arrayOf("loadfile", mpvPath(uri), if (i == 0) "replace" else "append"))
            }
        }
    }

    private val surfaceCallback = object : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) {
            if (!alive) return
            MPVLib.attachSurface(holder.surface)
            MPVLib.setOptionString("force-window", "yes")
            MPVLib.setPropertyString("vo", "gpu")
            loadFilesOnce()
        }
        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
            if (!alive) return
            MPVLib.setPropertyString("android-surface-size", "${width}x$height")
        }
        override fun surfaceDestroyed(holder: SurfaceHolder) {
            if (!alive) return
            MPVLib.setPropertyString("vo", "null")
            MPVLib.setOptionString("force-window", "no")
            MPVLib.detachSurface()
        }
    }

    // mpv calls these on its own thread.
    override fun eventProperty(property: String) {}
    override fun eventProperty(property: String, value: Long) {
        if (property == "playlist-pos" && value >= 0) runOnUiThread {
            if (value.toInt() != playlistPos.intValue) { runCatching { saveFrames() }; synchronized(frames) { frames.clear() } }
            playlistPos.intValue = value.toInt()
            items.getOrNull(value.toInt())?.let { title.value = it.name }
        }
    }
    override fun eventProperty(property: String, value: Double) {
        when (property) {
            "time-pos" -> position.doubleValue = value
            "duration" -> duration.doubleValue = value
            "demuxer-cache-time" -> cached.doubleValue = value
        }
    }
    override fun eventProperty(property: String, value: Boolean) {
        when (property) {
            "pause" -> { paused.value = value; runOnUiThread { updatePip() } }
            "paused-for-cache" -> { cacheWait = value; updateBuffering() }
            "seeking" -> { seekWait = value; updateBuffering() }
            "idle-active" -> if (value && firstFileLoaded) runOnUiThread { close() }
        }
    }
    override fun eventProperty(property: String, value: String) {}
    override fun event(eventId: Int) {
        when (eventId) {
            MPVLib.MPV_EVENT_FILE_LOADED -> {
                if (!firstFileLoaded) {
                    firstFileLoaded = true
                    // Later episodes start from the beginning.
                    MPVLib.setPropertyString("start", "none")
                    subtitles.forEachIndexed { i, s ->
                        MPVLib.command(arrayOf("sub-add", mpvPath(s), if (i == 0) "select" else "auto"))
                    }
                }
                loading = false; updateBuffering()
            }
            MPVLib.MPV_EVENT_START_FILE -> { loading = true; updateBuffering() }
            MPVLib.MPV_EVENT_PLAYBACK_RESTART -> { loading = false; seekWait = false; updateBuffering() }
            MPVLib.MPV_EVENT_VIDEO_RECONFIG -> runOnUiThread { updatePip() }
        }
    }

    data class Track(val id: String, val name: String, val detail: String, val selected: Boolean)

    /** Audio or subtitle tracks from mpv's track-list; subtitles get an "Off" row. */
    fun tracks(type: String): List<Track> {
        if (!alive) return emptyList()
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

    fun selectTrack(type: String, id: String) =
        MPVLib.setPropertyString(if (type == "audio") "aid" else "sid", id)

    fun togglePause() { if (alive) MPVLib.command(arrayOf("cycle", "pause")) }
    fun seekBy(seconds: Int) = MPVLib.command(arrayOf("seek", seconds.toString(), "relative+keyframes"))
    fun seekTo(seconds: Double) = MPVLib.command(arrayOf("seek", "%.3f".format(java.util.Locale.ROOT, seconds), "absolute+keyframes"))
    fun cycle(what: String) = MPVLib.command(arrayOf("osd-msg", "cycle", what))
    fun next() = MPVLib.command(arrayOf("playlist-next"))

    private fun close() {
        if (finishing) return
        runCatching { captureFrame(); saveFrames() }
        finishing = true
        val item = items.getOrNull(playlistPos.intValue) ?: items.firstOrNull()
        val result = Intent().apply {
            item?.let { data = it.uri }
            putExtra("position", (position.doubleValue * 1000).toLong())
            putExtra("duration", (duration.doubleValue * 1000).toLong())
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

    private fun pipParams(): android.app.PictureInPictureParams? {
        if (Build.VERSION.SDK_INT < 26 || !alive) return null
        val aspect = runCatching { MPVLib.getPropertyDouble("video-params/aspect") }.getOrNull()?.takeIf { it > 0 } ?: (16.0 / 9)
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
            if (Build.VERSION.SDK_INT >= 31) setAutoEnterEnabled(!finishing).setSeamlessResizeEnabled(true)
        }.build()
    }

    fun enterPip() {
        if (Build.VERSION.SDK_INT < 26) return
        if (!packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_PICTURE_IN_PICTURE)) return
        pipParams()?.let { runCatching { enterPictureInPictureMode(it) } }
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
        // Closed from the small window: stop and report the position.
        if (!isInPictureInPictureMode && lifecycle.currentState == androidx.lifecycle.Lifecycle.State.CREATED) close()
    }

    override fun onStop() {
        super.onStop()
        if (!isFinishing && alive) MPVLib.setPropertyBoolean("pause", true)
    }

    override fun finish() {
        if (!finishing) {
            runCatching { saveFrames() }
            // System back or idle end: still report where playback stopped.
            finishing = true
            val item = items.getOrNull(playlistPos.intValue) ?: items.firstOrNull()
            setResult(Activity.RESULT_OK, Intent().apply {
                item?.let { data = it.uri }
                putExtra("position", (position.doubleValue * 1000).toLong())
                putExtra("duration", (duration.doubleValue * 1000).toLong())
            })
        }
        teardown() // free the player now, not later in onDestroy
        super.finish()
    }

    override fun onDestroy() {
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

    // ---- Library previews from the player ----

    private val frames = ArrayList<android.graphics.Bitmap>()

    /** Grab the current picture through mpv, scaled down for the library card. */
    private fun captureFrame() {
        if (!alive) return
        if (duration.doubleValue <= 0 || position.doubleValue < 3) return
        val file = java.io.File(cacheDir, "player-frame.jpg")
        file.delete()
        MPVLib.command(arrayOf("screenshot-to-file", file.path, "video"))
        if (!file.exists() || file.length() == 0L) return
        val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
        android.graphics.BitmapFactory.decodeFile(file.path, bounds)
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= 480) sample *= 2
        val bmp = android.graphics.BitmapFactory.decodeFile(file.path, android.graphics.BitmapFactory.Options().apply { inSampleSize = sample })
        file.delete()
        if (bmp != null) synchronized(frames) { frames += bmp; while (frames.size > 3) frames.removeAt(0) }
    }

    private fun saveFrames() {
        val item = items.getOrNull(playlistPos.intValue) ?: return
        val (id, index) = torrentOf(item.uri) ?: return
        val entry = libraryEntry(id) ?: return
        val video = entry.files.find { it.index == index } ?: return
        val list = synchronized(frames) { frames.toList() }
        (application as WebtorApp).thumbnails.putPlayerFrames(entry, video, list)
    }

    // ---- Volume, brightness, speed ----

    private val audio by lazy { getSystemService(Context.AUDIO_SERVICE) as android.media.AudioManager }

    private fun currentBrightness(): Float {
        val w = window.attributes.screenBrightness
        if (w >= 0) return w
        return runCatching {
            android.provider.Settings.System.getInt(contentResolver, android.provider.Settings.System.SCREEN_BRIGHTNESS) / 255f
        }.getOrDefault(0.5f)
    }

    private fun setBrightness(v: Float) {
        window.attributes = window.attributes.apply { screenBrightness = v.coerceIn(0.01f, 1f) }
    }

    private fun volumeFraction(): Float {
        val max = audio.getStreamMaxVolume(android.media.AudioManager.STREAM_MUSIC).coerceAtLeast(1)
        return audio.getStreamVolume(android.media.AudioManager.STREAM_MUSIC).toFloat() / max
    }

    private fun setVolume(v: Float) {
        val max = audio.getStreamMaxVolume(android.media.AudioManager.STREAM_MUSIC)
        audio.setStreamVolume(android.media.AudioManager.STREAM_MUSIC, (v.coerceIn(0f, 1f) * max).roundToInt(), 0)
    }

    private fun setSpeed(x: Double) = MPVLib.setPropertyDouble("speed", x)

    private val aspect by lazy { mutableStateOf(PlayerAspect.load(this)) }
    private val rotationLocked = mutableStateOf(false)

    /** Fit keeps the whole picture, Crop fills the screen, Stretch ignores the video's shape. */
    private fun applyAspect(mode: PlayerAspect) {
        MPVLib.setPropertyString("keepaspect", if (mode == PlayerAspect.STRETCH) "no" else "yes")
        MPVLib.setPropertyString("panscan", if (mode == PlayerAspect.CROP) "1.0" else "0.0")
        MPVLib.setPropertyString("video-aspect-override", when (mode) {
            PlayerAspect.R16_9 -> "16:9"
            PlayerAspect.R4_3 -> "4:3"
            else -> "-1"
        })
    }

    private fun cycleAspect(): PlayerAspect {
        val mode = aspect.value.next()
        aspect.value = mode
        PlayerAspect.save(this, mode)
        applyAspect(mode)
        return mode
    }

    /** Lock keeps the side the screen is on now; unlocked follows the sensor between both landscape sides. */
    private fun toggleRotationLock(): Boolean {
        val lock = !rotationLocked.value
        rotationLocked.value = lock
        requestedOrientation = if (lock) ActivityInfo.SCREEN_ORIENTATION_LOCKED else ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        return lock
    }

    private enum class Drag { NONE, VOLUME, BRIGHTNESS, SEEK, SPEED, IGNORE }

    @Composable
    private fun PlayerScreen(surface: (Context) -> SurfaceView, onBack: () -> Unit) {
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
        // Opened at a saved spot: say so for a few seconds, with a way back to the start.
        var resumeChip by remember { mutableStateOf(startSeconds > 5) }

        BackHandler(onBack = onBack)
        LaunchedEffect(Unit) { if (resumeChip) { delay(10_000); resumeChip = false } }
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
            delay(8000)
            while (true) {
                if (!paused.value && !buffering.value) withContext(Dispatchers.IO) { runCatching { captureFrame() } }
                delay(90_000)
            }
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
        val flashScope = androidx.compose.runtime.rememberCoroutineScope()
        var flashJob by remember { mutableStateOf<kotlinx.coroutines.Job?>(null) }
        fun flash(text: String) {
            popup = text; popupSub = null; popupLevel = null
            flashJob?.cancel()
            flashJob = flashScope.launch { delay(1200); if (popup == text) popup = null }
        }

        Box(Modifier.fillMaxSize().background(Color.Black)) {
            AndroidView(factory = surface, modifier = Modifier.fillMaxSize())

            if (inPip.value) return@Box
            // Gesture layer: tap, double tap, hold for 2x, swipes.
            Box(
                Modifier.fillMaxSize()
                    .pointerInput(Unit) {
                        var held = false
                        var streakPress = false
                        fun sideOf(x: Float) = when { x < size.width / 3f -> -1; x > size.width * 2f / 3f -> 1; else -> 0 }
                        detectTapGestures(
                            onPress = { o ->
                                held = false
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
                            onTap = { if (!held && !streakPress) { controls = !controls; touchTick++ } },
                            onDoubleTap = { o ->
                                if (!streakPress) {
                                    val side = sideOf(o.x)
                                    if (side == 0) togglePause()
                                    else { seekBy(10 * side); skipSide = side; skipTotal = 10; skipTick++ }
                                }
                            },
                        )
                    }
                    .pointerInput(Unit) {
                        // Swipes as in mpvEx: measured from where the finger went down.
                        // Clearly sideways (over 30 px and twice the vertical move) seeks
                        // 0.05 s per pixel with the video paused, and resumes on release.
                        // Up/down: left half brightness, right half volume.
                        awaitEachGesture {
                            val down = awaitFirstDown(requireUnconsumed = false)
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
                                    if (!wasPaused) MPVLib.setPropertyBoolean("pause", false)
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
                                            if (!wasPaused) MPVLib.setPropertyBoolean("pause", true)
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
                                        val v = (level - dy / (size.height * 0.8f)).coerceIn(0.01f, 1f)
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
            if (isBuffering && !isPaused && !controls) {
                Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator(Modifier.size(44.dp), color = Color.White, strokeWidth = 3.dp)
                    swarm.value?.let {
                        Spacer(Modifier.height(12.dp))
                        Text(it, color = Color.White, fontSize = 13.sp,
                            modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(Color.Black.copy(alpha = 0.5f)).padding(horizontal = 10.dp, vertical = 4.dp))
                    }
                }
            }

            AnimatedVisibility(controls, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
                Box(Modifier.fillMaxSize()) {
                    // Shade only top and bottom so the picture stays clear.
                    Box(Modifier.fillMaxWidth().height(110.dp).align(Alignment.TopCenter)
                        .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.7f), Color.Transparent))))
                    Box(Modifier.fillMaxWidth().height(170.dp).align(Alignment.BottomCenter)
                        .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.8f)))))

                    // Top: back and title only.
                    Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White)
                        }
                        Column(Modifier.weight(1f).padding(start = 4.dp)) {
                            Text(title.value, color = Color.White, style = MaterialTheme.typography.titleMedium,
                                maxLines = 1, overflow = TextOverflow.Ellipsis)
                            // Downloads show progress; Direct Play keeps video in RAM, so a percentage means nothing there.
                            val lead = if (directPlay.value) "Direct Play · streaming from RAM" else fileDone.value?.let { "$it% downloaded" }
                            val info = listOfNotNull(lead, swarm.value).joinToString("  ·  ")
                            if (info.isNotEmpty()) Text(info, color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelMedium)
                        }
                        IconButton(onClick = { trackSheet = "audio"; controls = false }) {
                            Icon(Icons.Filled.Audiotrack, "Audio", tint = Color.White)
                        }
                        IconButton(onClick = { trackSheet = "sub"; controls = false }) {
                            Icon(Icons.Filled.Subtitles, "Subtitles", tint = Color.White)
                        }
                        IconButton(onClick = { flash(cycleAspect().label); touchTick++ }) {
                            Icon(Icons.Filled.AspectRatio, "Aspect ratio: ${aspect.value.label}", tint = Color.White)
                        }
                        IconButton(onClick = { flash(if (toggleRotationLock()) "Rotation locked" else "Auto-rotate"); touchTick++ }) {
                            Icon(
                                if (rotationLocked.value) Icons.Filled.ScreenLockRotation else Icons.Filled.ScreenRotation,
                                if (rotationLocked.value) "Unlock rotation" else "Lock rotation",
                                tint = Color.White,
                            )
                        }
                        if (items.size > 1 && playlistPos.intValue < items.size - 1) {
                            IconButton(onClick = { next(); touchTick++ }) {
                                Icon(Icons.Filled.SkipNext, "Next episode", tint = Color.White)
                            }
                        }
                    }

                    // Center: rewind, play/pause, forward.
                    Row(Modifier.align(Alignment.Center), horizontalArrangement = Arrangement.spacedBy(56.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = { seekBy(-10); touchTick++ }, modifier = Modifier.size(56.dp)) {
                            Icon(Icons.Filled.Replay10, "Back 10 seconds", tint = Color.White, modifier = Modifier.size(36.dp))
                        }
                        androidx.compose.material3.FilledTonalIconButton(
                            onClick = { togglePause(); touchTick++ },
                            modifier = Modifier.size(72.dp),
                            colors = androidx.compose.material3.IconButtonDefaults.filledTonalIconButtonColors(
                                containerColor = Color.Black.copy(alpha = 0.4f), contentColor = Color.White),
                        ) {
                            when {
                                isBuffering && !isPaused -> CircularProgressIndicator(Modifier.size(40.dp), color = Color.White, strokeWidth = 3.dp)
                                isPaused -> Icon(Icons.Filled.PlayArrow, "Play", modifier = Modifier.size(44.dp))
                                else -> Icon(Icons.Filled.Pause, "Pause", modifier = Modifier.size(44.dp))
                            }
                        }
                        IconButton(onClick = { seekBy(10); touchTick++ }, modifier = Modifier.size(56.dp)) {
                            Icon(Icons.Filled.Forward10, "Forward 10 seconds", tint = Color.White, modifier = Modifier.size(36.dp))
                        }
                    }

                    // Bottom: time, seek bar, then actions.
                    Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(start = 28.dp, end = 28.dp, top = 14.dp, bottom = 40.dp)) {
                        val shown = when {
                            sliderDragging -> sliderValue.toDouble()
                            else -> pendingSeek ?: pos
                        }
                        if (Build.VERSION.SDK_INT >= 26) {
                            IconButton(onClick = { controls = false; enterPip() }, modifier = Modifier.align(Alignment.End)) {
                                Icon(Icons.Filled.PictureInPictureAlt, "Picture in picture", tint = Color.White)
                            }
                        }
                        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp)) {
                            Text(clock(shown), color = Color.White, style = MaterialTheme.typography.labelLarge)
                            Spacer(Modifier.weight(1f))
                            val ahead = (cached.doubleValue - pos).coerceAtLeast(0.0)
                            if (ahead >= 1) Text("buffered ${clock(ahead)}   ", color = Color.White.copy(alpha = 0.65f),
                                style = MaterialTheme.typography.labelLarge)
                            Text(clock(dur), color = Color.White.copy(alpha = 0.8f), style = MaterialTheme.typography.labelLarge)
                        }
                        SeekBar(
                            fraction = if (dur > 0) (shown / dur).toFloat().coerceIn(0f, 1f) else 0f,
                            cachedFraction = if (dur > 0) (cached.doubleValue / dur).toFloat().coerceIn(0f, 1f) else 0f,
                            downloaded = if (directPlay.value) emptyList() else spans.value,
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

            trackSheet?.let { type ->
                TrackSheet(type, onDismiss = { trackSheet = null })
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

            // Resumed from the saved spot, with Start over.
            if (resumeChip) {
                Row(
                    Modifier.align(Alignment.BottomStart).padding(start = 28.dp, bottom = 132.dp)
                        .clip(RoundedCornerShape(16.dp)).background(Color.Black.copy(alpha = 0.6f))
                        .padding(start = 14.dp, end = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Resumed at ${clock(startSeconds)}", color = Color.White, style = MaterialTheme.typography.labelLarge)
                    TextButton(onClick = {
                        if (firstFileLoaded) seekTo(0.0) else MPVLib.setPropertyString("start", "0")
                        resumeChip = false; touchTick++
                    }) { Text("Start over", color = Color.White) }
                }
            }

            // Small pop-up for swipes and seeking.
            popup?.let { text ->
                Column(
                    Modifier.align(Alignment.TopCenter).padding(top = 72.dp)
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

            // Hold speed: chip at the top with the current speed. Locked speed stays; tap the chip for normal speed.
            if (fast || speedLocked) {
                Column(
                    Modifier.align(Alignment.TopCenter).padding(top = 24.dp)
                        .clip(RoundedCornerShape(16.dp)).background(Color.Black.copy(alpha = 0.55f))
                        .then(if (speedLocked && !fast) Modifier.clickable { speedLocked = false; holdSpeed = 2.0; setSpeed(1.0) } else Modifier)
                        .padding(horizontal = 14.dp, vertical = 6.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(speedLabel(holdSpeed), color = Color.White, style = MaterialTheme.typography.titleSmall)
                        Spacer(Modifier.width(4.dp))
                        Icon(if (speedLocked) Icons.Filled.Lock else Icons.Filled.FastForward, null, tint = Color.White, modifier = Modifier.size(16.dp))
                    }
                    Text(
                        when {
                            fast && !speedLocked -> "slide ← → to change · swipe up to keep"
                            fast -> "speed kept"
                            else -> "tap for normal speed"
                        },
                        color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.labelSmall,
                    )
                }
            }
        }
    }

    /**
     * Seek bar: dim track, green where the torrent has the file downloaded
     * (fainter green where it is still downloading), light where the player has
     * it buffered, white for what was played.
     */
    @Composable
    private fun SeekBar(fraction: Float, cachedFraction: Float, downloaded: List<Span>, onDrag: (Float) -> Unit, onDragEnd: () -> Unit) {
        var active by remember { mutableStateOf(false) }
        Canvas(
            Modifier.fillMaxWidth().height(32.dp)
                .pointerInput(Unit) {
                    detectTapGestures(
                        onPress = { o -> active = true; onDrag((o.x / size.width).coerceIn(0f, 1f)); val tap = tryAwaitRelease(); if (tap) { active = false; onDragEnd() } },
                    )
                }
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = { o -> active = true; onDrag((o.x / size.width).coerceIn(0f, 1f)) },
                        onDragEnd = { active = false; onDragEnd() },
                        onDragCancel = { active = false; onDragEnd() },
                    ) { change, _ -> change.consume(); onDrag((change.position.x / size.width).coerceIn(0f, 1f)) }
                },
        ) {
            val h = (if (active) 6.dp else 4.dp).toPx()
            val y = size.height / 2 - h / 2
            val r = CornerRadius(h / 2)
            drawRoundRect(Color.White.copy(alpha = 0.22f), Offset(0f, y), Size(size.width, h), r)
            downloaded.forEach { s ->
                if (s.done <= 0f) return@forEach
                val green = Color(0xFF4CAF50).copy(alpha = if (s.done >= 0.99f) 0.85f else 0.25f + 0.45f * s.done)
                val x = s.from * size.width
                drawRect(green, Offset(x, y), Size(((s.to - s.from) * size.width).coerceAtLeast(1f), h))
            }
            if (cachedFraction > fraction) drawRect(Color.White.copy(alpha = 0.45f),
                Offset(fraction * size.width, y), Size((cachedFraction - fraction) * size.width, h))
            drawRoundRect(Color.White, Offset(0f, y), Size(fraction * size.width, h), r)
            drawCircle(Color.White, radius = (if (active) 9.dp else 7.dp).toPx(), center = Offset(fraction * size.width, size.height / 2))
        }
    }

    /** Material panel to pick an audio or subtitle track. */
    @Composable
    private fun TrackSheet(type: String, onDismiss: () -> Unit) {
        var list by remember(type) { mutableStateOf(tracks(type)) }
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
                        Icon(if (type == "audio") Icons.Filled.Audiotrack else Icons.Filled.Subtitles, null,
                            tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.width(12.dp))
                        Text(if (type == "audio") "Audio" else "Subtitles", style = MaterialTheme.typography.titleMedium,
                            color = MaterialTheme.colorScheme.onSurface, modifier = Modifier.weight(1f))
                        val count = if (type == "sub") list.size - 1 else list.size
                        if (count > 0) Text("$count", style = MaterialTheme.typography.labelLarge,
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
                            val pick = { selectTrack(type, t.id); list = tracks(type); onDismiss() }
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

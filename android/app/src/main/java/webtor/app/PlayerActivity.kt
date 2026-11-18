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
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
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
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.height
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.text.font.FontWeight
import dev.jdtech.mpv.MPVLib
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.roundToInt

/** Settings flag for the built-in mpv player (experimental, on by default in this test build). */
object BuiltInPlayer {
    private const val KEY = "builtInPlayer"
    fun enabled(context: Context): Boolean =
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).getBoolean(KEY, true)
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
    private val playlistPos = mutableIntStateOf(0)

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

        setContent {
            PlayerScreen(
                surface = { SurfaceView(it).apply { holder.addCallback(surfaceCallback) } },
                onBack = ::close,
            )
        }
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
        MPVLib.create(applicationContext)
        listOf(
            "vo" to "gpu",
            "gpu-context" to "android",
            "opengl-es" to "yes",
            "hwdec" to "mediacodec,mediacodec-copy",
            "hwdec-codecs" to "h264,hevc,mpeg4,mpeg2video,vp8,vp9,av1",
            "ao" to "audiotrack,opensles",
            "audio-set-media-role" to "yes",
            "tls-verify" to "no",
            "cache" to "yes",
            "demuxer-max-bytes" to "64MiB",
            "demuxer-max-back-bytes" to "32MiB",
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
        ).forEach { (k, v) -> MPVLib.setOptionString(k, v) }
        MPVLib.init()
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
        items.forEachIndexed { i, item ->
            MPVLib.command(arrayOf("loadfile", mpvPath(item.uri), if (i == 0) "replace" else "append"))
        }
    }

    private val surfaceCallback = object : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) {
            MPVLib.attachSurface(holder.surface)
            MPVLib.setOptionString("force-window", "yes")
            MPVLib.setPropertyString("vo", "gpu")
        }
        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
            MPVLib.setPropertyString("android-surface-size", "${width}x$height")
        }
        override fun surfaceDestroyed(holder: SurfaceHolder) {
            MPVLib.setPropertyString("vo", "null")
            MPVLib.setOptionString("force-window", "no")
            MPVLib.detachSurface()
        }
    }

    // mpv calls these on its own thread.
    override fun eventProperty(property: String) {}
    override fun eventProperty(property: String, value: Long) {
        if (property == "playlist-pos" && value >= 0) runOnUiThread {
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
            "pause" -> paused.value = value
            "paused-for-cache", "seeking" -> buffering.value = value
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
                buffering.value = false
            }
            MPVLib.MPV_EVENT_PLAYBACK_RESTART -> buffering.value = false
        }
    }

    fun togglePause() = MPVLib.command(arrayOf("cycle", "pause"))
    fun seekBy(seconds: Int) = MPVLib.command(arrayOf("seek", seconds.toString(), "relative+keyframes"))
    fun seekTo(seconds: Double) = MPVLib.command(arrayOf("seek", "%.3f".format(java.util.Locale.ROOT, seconds), "absolute+keyframes"))
    fun cycle(what: String) = MPVLib.command(arrayOf("osd-msg", "cycle", what))
    fun next() = MPVLib.command(arrayOf("playlist-next"))

    private fun close() {
        if (finishing) return
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

    override fun onStop() {
        super.onStop()
        if (!isFinishing) MPVLib.setPropertyBoolean("pause", true)
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
            })
        }
        super.finish()
    }

    override fun onDestroy() {
        MPVLib.removeObserver(this)
        MPVLib.destroy()
        super.onDestroy()
    }

    // ---- Torrent info for the playing file (http stream only) ----

    /** One stretch of the file with the share of verified pieces in it, positions 0..1. */
    data class Span(val from: Float, val to: Float, val done: Float)

    private val spans = mutableStateOf<List<Span>>(emptyList())
    private val swarm = mutableStateOf<String?>(null)

    /** Engine id and file index from http://127.0.0.1:port/torrent/<id>/file/<n>. */
    private fun torrentOf(uri: Uri): Pair<String, Int>? {
        if (uri.scheme != "http") return null
        val seg = uri.pathSegments
        val t = seg.indexOf("torrent")
        if (t < 0 || seg.size < t + 4 || seg[t + 2] != "file") return null
        return seg[t + 1] to (seg[t + 3].toIntOrNull() ?: return null)
    }

    private fun refreshTorrentInfo() {
        val item = items.getOrNull(playlistPos.intValue) ?: return
        val (id, fileIndex) = torrentOf(item.uri) ?: run { spans.value = emptyList(); swarm.value = null; return }
        val client = (application as WebtorApp).engineClient
        val status = runCatching { client.torrent(id) }.getOrNull() ?: return
        swarm.value = "${status.numPeers} peers · ${formatSpeed(status.downloadSpeed)}"
        val files = status.files.sortedBy { it.index }
        val file = files.find { it.index == fileIndex } ?: return
        if (file.length <= 0) return
        if (file.progress >= 1.0) { spans.value = listOf(Span(0f, 1f, 1f)); return }
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

    private enum class Drag { NONE, VOLUME, BRIGHTNESS, SEEK }

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
        val pos by position
        val dur by duration
        val isPaused by paused
        val isBuffering by buffering

        BackHandler(onBack = onBack)
        LaunchedEffect(controls, touchTick, isPaused, sliderDragging) {
            if (controls && !isPaused && !sliderDragging) { delay(3500); controls = false }
        }
        LaunchedEffect(Unit) {
            while (true) {
                withContext(Dispatchers.IO) { refreshTorrentInfo() }
                delay(1000)
            }
        }

        fun seekPopup(target: Double) {
            val delta = target - pos
            popup = (if (delta >= 0) "+" else "-") + clock(kotlin.math.abs(delta)) + "   " + clock(target)
            popupSub = when (if (dur > 0) readyAt((target / dur).toFloat()) else null) {
                true -> "Downloaded · plays instantly"
                false -> "Not downloaded yet · will buffer"
                null -> null
            }
            popupLevel = null
        }

        Box(Modifier.fillMaxSize().background(Color.Black)) {
            AndroidView(factory = surface, modifier = Modifier.fillMaxSize())

            // Gesture layer: tap, double tap, hold for 2x, swipes.
            Box(
                Modifier.fillMaxSize()
                    .pointerInput(Unit) {
                        var held = false
                        detectTapGestures(
                            onPress = {
                                held = false
                                coroutineScope {
                                    val hold = launch {
                                        delay(450)
                                        held = true; fast = true; setSpeed(2.0)
                                        popup = "2× speed"; popupSub = "Release to go back"; popupLevel = null
                                    }
                                    tryAwaitRelease()
                                    hold.cancel()
                                    if (held) { setSpeed(1.0); fast = false; popup = null; popupSub = null }
                                }
                            },
                            onTap = { if (!held) { controls = !controls; touchTick++ } },
                            onDoubleTap = { o ->
                                when {
                                    o.x < size.width / 3f -> { seekBy(-10); popup = "-10 s"; popupSub = null; popupLevel = null }
                                    o.x > size.width * 2f / 3f -> { seekBy(10); popup = "+10 s"; popupSub = null; popupLevel = null }
                                    else -> togglePause()
                                }
                                touchTick++
                            },
                        )
                    }
                    .pointerInput(Unit) {
                        var mode = Drag.NONE
                        var startX = 0f
                        var level = 0f
                        var target = 0.0
                        var total = Offset.Zero
                        detectDragGestures(
                            onDragStart = { o -> mode = Drag.NONE; startX = o.x; total = Offset.Zero },
                            onDragEnd = {
                                if (mode == Drag.SEEK) seekTo(target)
                                mode = Drag.NONE; popup = null; popupSub = null; popupLevel = null
                            },
                            onDragCancel = { mode = Drag.NONE; popup = null; popupSub = null; popupLevel = null },
                        ) { change, d ->
                            change.consume()
                            total += d
                            if (mode == Drag.NONE) {
                                mode = when {
                                    kotlin.math.abs(total.x) > kotlin.math.abs(total.y) -> Drag.SEEK
                                    startX < size.width / 2f -> Drag.VOLUME
                                    else -> Drag.BRIGHTNESS
                                }
                                level = when (mode) { Drag.VOLUME -> volumeFraction(); Drag.BRIGHTNESS -> currentBrightness(); else -> 0f }
                                target = pos
                            }
                            when (mode) {
                                Drag.VOLUME -> {
                                    level = (level - d.y / (size.height * 0.8f)).coerceIn(0f, 1f)
                                    setVolume(level)
                                    popup = "Volume ${(level * 100).roundToInt()}%"; popupSub = null; popupLevel = level
                                }
                                Drag.BRIGHTNESS -> {
                                    level = (level - d.y / (size.height * 0.8f)).coerceIn(0.01f, 1f)
                                    setBrightness(level)
                                    popup = "Brightness ${(level * 100).roundToInt()}%"; popupSub = null; popupLevel = level
                                }
                                Drag.SEEK -> {
                                    // A full screen width is about 2 minutes.
                                    target = (target + d.x / size.width * 120.0).coerceIn(0.0, if (dur > 0) dur else Double.MAX_VALUE)
                                    seekPopup(target)
                                }
                                Drag.NONE -> {}
                            }
                        }
                    },
            )

            if (isBuffering && !isPaused) {
                Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator(Modifier.size(48.dp), color = Color.White)
                    swarm.value?.let {
                        Spacer(Modifier.height(10.dp))
                        Text("Buffering · $it", color = Color.White, fontSize = 13.sp,
                            modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(Color.Black.copy(alpha = 0.5f)).padding(horizontal = 10.dp, vertical = 4.dp))
                    }
                }
            }

            AnimatedVisibility(controls, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.35f))) {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White)
                        }
                        Column(Modifier.weight(1f)) {
                            Text(title.value, color = Color.White, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            swarm.value?.let { Text(it, color = Color.White.copy(alpha = 0.7f), fontSize = 12.sp) }
                        }
                        PillButton("Audio") { cycle("aid"); touchTick++ }
                        PillButton("Subs") { cycle("sid"); touchTick++ }
                        if (items.size > 1 && playlistPos.intValue < items.size - 1) PillButton("Next") { next(); touchTick++ }
                    }
                    Row(Modifier.align(Alignment.Center), horizontalArrangement = Arrangement.spacedBy(40.dp), verticalAlignment = Alignment.CenterVertically) {
                        RoundButton("-10") { seekBy(-10); touchTick++ }
                        Box(
                            Modifier.size(72.dp).clip(CircleShape).background(Color.White.copy(alpha = 0.2f))
                                .pointerInput(Unit) { detectTapGestures { togglePause(); touchTick++ } },
                            contentAlignment = Alignment.Center,
                        ) {
                            if (isPaused) Icon(Icons.Filled.PlayArrow, "Play", tint = Color.White, modifier = Modifier.size(44.dp))
                            else PauseGlyph()
                        }
                        RoundButton("+10") { seekBy(10); touchTick++ }
                    }
                    Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(horizontal = 32.dp, vertical = 16.dp)) {
                        val shown = if (sliderDragging) sliderValue.toDouble() else pos
                        Slider(
                            value = if (dur > 0) (shown / dur).toFloat().coerceIn(0f, 1f) else 0f,
                            onValueChange = {
                                sliderDragging = true; sliderValue = (it * dur).toFloat()
                                seekPopup(sliderValue.toDouble())
                            },
                            onValueChangeFinished = {
                                seekTo(sliderValue.toDouble()); sliderDragging = false; popup = null; popupSub = null; touchTick++
                            },
                            colors = SliderDefaults.colors(thumbColor = Color.White, activeTrackColor = Color.White,
                                inactiveTrackColor = Color.White.copy(alpha = 0.25f)),
                        )
                        DownloadedBar(Modifier.fillMaxWidth().padding(horizontal = 10.dp).height(4.dp))
                        Spacer(Modifier.height(6.dp))
                        Row(Modifier.fillMaxWidth()) {
                            Text(clock(shown), color = Color.White, fontSize = 13.sp)
                            Spacer(Modifier.weight(1f))
                            val ahead = (cached.doubleValue - pos).coerceAtLeast(0.0)
                            if (ahead > 0) Text("buffered ${ahead.toInt()} s  ", color = Color.White.copy(alpha = 0.6f), fontSize = 13.sp)
                            Text(clock(dur), color = Color.White, fontSize = 13.sp)
                        }
                    }
                }
            }

            // Center pop-up for gestures and seeking.
            popup?.let { text ->
                Column(
                    Modifier.align(Alignment.TopCenter).padding(top = 56.dp)
                        .clip(RoundedCornerShape(14.dp)).background(Color.Black.copy(alpha = 0.7f))
                        .padding(horizontal = 20.dp, vertical = 12.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(text, color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                    popupLevel?.let { lv ->
                        Spacer(Modifier.height(8.dp))
                        Box(Modifier.width(160.dp).height(4.dp).clip(RoundedCornerShape(2.dp)).background(Color.White.copy(alpha = 0.25f))) {
                            Box(Modifier.fillMaxWidth(lv).height(4.dp).background(Color.White))
                        }
                    }
                    popupSub?.let {
                        Spacer(Modifier.height(4.dp))
                        Text(it, color = if (it.startsWith("Downloaded")) Color(0xFF7CE38B) else Color.White.copy(alpha = 0.75f), fontSize = 13.sp)
                    }
                }
            }
            if (fast && popup == null) {
                Text("2×", color = Color.White, fontSize = 16.sp, modifier = Modifier.align(Alignment.TopCenter).padding(top = 56.dp))
            }
        }
    }

    /** Which parts of this file are downloaded: green where verified, dim where missing. */
    @Composable
    private fun DownloadedBar(modifier: Modifier) {
        val list = spans.value
        if (list.isEmpty()) return
        Canvas(modifier.clip(RoundedCornerShape(2.dp))) {
            drawRect(Color.White.copy(alpha = 0.15f))
            list.forEach { sp ->
                if (sp.done > 0f) drawRect(
                    Color(0xFF7CE38B).copy(alpha = 0.35f + 0.65f * sp.done),
                    topLeft = Offset(sp.from * size.width, 0f),
                    size = Size(((sp.to - sp.from) * size.width).coerceAtLeast(1f), size.height),
                )
            }
        }
    }

    @Composable
    private fun PillButton(label: String, onClick: () -> Unit) {
        TextButton(onClick = onClick) { Text(label, color = Color.White) }
    }

    @Composable
    private fun RoundButton(label: String, onClick: () -> Unit) {
        Box(
            Modifier.size(56.dp).clip(CircleShape).background(Color.White.copy(alpha = 0.15f))
                .pointerInput(Unit) { detectTapGestures { onClick() } },
            contentAlignment = Alignment.Center,
        ) { Text(label, color = Color.White, fontSize = 15.sp) }
    }

    @Composable
    private fun PauseGlyph() {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            repeat(2) { Box(Modifier.size(8.dp, 28.dp).clip(RoundedCornerShape(2.dp)).background(Color.White)) }
        }
    }
}

private fun clock(seconds: Double): String {
    val s = seconds.toLong().coerceAtLeast(0)
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

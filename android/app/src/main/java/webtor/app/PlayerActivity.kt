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
import dev.jdtech.mpv.MPVLib
import kotlinx.coroutines.delay

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

    @Composable
    private fun PlayerScreen(surface: (Context) -> SurfaceView, onBack: () -> Unit) {
        var controls by remember { mutableStateOf(true) }
        var touchTick by remember { mutableIntStateOf(0) }
        var dragging by remember { mutableStateOf(false) }
        var dragValue by remember { mutableFloatStateOf(0f) }
        val pos by position
        val dur by duration
        val isPaused by paused
        val isBuffering by buffering

        BackHandler(onBack = onBack)
        LaunchedEffect(controls, touchTick, isPaused, dragging) {
            if (controls && !isPaused && !dragging) { delay(3500); controls = false }
        }

        Box(Modifier.fillMaxSize().background(Color.Black)) {
            AndroidView(factory = surface, modifier = Modifier.fillMaxSize())
            Box(
                Modifier.fillMaxSize().pointerInput(Unit) {
                    detectTapGestures(
                        onTap = { controls = !controls; touchTick++ },
                        onDoubleTap = { o ->
                            when {
                                o.x < size.width / 3f -> seekBy(-10)
                                o.x > size.width * 2f / 3f -> seekBy(10)
                                else -> togglePause()
                            }
                            touchTick++
                        },
                    )
                },
            )
            if (isBuffering) {
                CircularProgressIndicator(Modifier.align(Alignment.Center).size(48.dp), color = Color.White)
            }
            AnimatedVisibility(controls, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.fillMaxSize()) {
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.35f))) {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White)
                        }
                        Text(title.value, color = Color.White, fontSize = 16.sp, maxLines = 1,
                            overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
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
                        val shown = if (dragging) dragValue.toDouble() else pos
                        Slider(
                            value = if (dur > 0) (shown / dur).toFloat().coerceIn(0f, 1f) else 0f,
                            onValueChange = { dragging = true; dragValue = (it * dur).toFloat() },
                            onValueChangeFinished = { seekTo(dragValue.toDouble()); dragging = false; touchTick++ },
                            colors = SliderDefaults.colors(thumbColor = Color.White, activeTrackColor = Color.White,
                                inactiveTrackColor = Color.White.copy(alpha = 0.3f)),
                        )
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
            repeat(2) { Box(Modifier.width(8.dp).size(8.dp, 28.dp).clip(RoundedCornerShape(2.dp)).background(Color.White)) }
        }
    }
}

private fun clock(seconds: Double): String {
    val s = seconds.toLong().coerceAtLeast(0)
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

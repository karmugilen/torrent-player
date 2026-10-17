package webtor.app

import android.graphics.Bitmap
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.delay

@Composable
fun rememberVideoPreviews(entry: DownloadEntry): List<Bitmap>? {
    val repository = (LocalContext.current.applicationContext as WebtorApp).thumbnails
    val owner = LocalLifecycleOwner.current
    val video = firstPreviewVideo(entry)
    val latest by rememberUpdatedState(entry)
    var frames by remember(entry.key, video?.index) { mutableStateOf(repository.cached(entry)) }
    LaunchedEffect(entry.key, video?.index, entry.complete, entry.paused, entry.engineId, owner) {
        if (video == null) return@LaunchedEffect
        owner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                val current = latest
                val next = repository.previews(current)
                if (next.isNotEmpty()) frames = next else if (frames == null) frames = emptyList()
                if (current.complete || current.paused || current.engineId == null) break
                delay(4_000L)
            }
        }
    }
    return frames
}

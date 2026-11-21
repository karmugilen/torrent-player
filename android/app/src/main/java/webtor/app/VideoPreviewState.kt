package webtor.app

import android.graphics.Bitmap
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.delay

@Composable
fun rememberVideoPreviews(entry: DownloadEntry, fileIndex: Int? = null): List<Bitmap>? {
    val repository = (LocalContext.current.applicationContext as WebtorApp).thumbnails
    val owner = LocalLifecycleOwner.current
    val video = previewVideo(entry, fileIndex)
    val identity = video?.let { ThumbnailRepository.cacheKey(entry, it) }
    val latest by rememberUpdatedState(entry)
    var frames by remember(identity) { mutableStateOf(repository.cached(entry, video?.index)) }
    LaunchedEffect(identity, video?.isVerifiedComplete, entry.complete, entry.paused, entry.engineId, owner) {
        if (video == null) return@LaunchedEffect
        owner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                val current = latest
                val next = repository.previews(current, video.index)
                if (next.isNotEmpty()) frames = next else if (frames == null) frames = emptyList()
                if ((!current.directPlay && previewVideo(current, video.index)?.isVerifiedComplete == true) ||
                    current.engineId == null) break
                // A paused download fetches nothing; a paused Direct Play may still be
                // reading its saved frames, so look until they show.
                if (current.paused && (!current.directPlay || next.isNotEmpty())) break
                // Waiting for the engine's keyframe: look again soon.
                delay(if (next.isEmpty()) (if (current.paused) 5_000L else 1_500L) else 4_000L)
            }
        }
    }
    return frames
}

/**
 * One video, or the episode being watched in a pack: the same three pictures
 * to swipe through either way. A pack adds a small "2/10" mark.
 */
@Composable
fun EpisodeVideoPreview(
    entry: DownloadEntry,
    modifier: Modifier = Modifier,
    large: Boolean = false,
    /** Drawn on top; gets the file index of the episode shown (null for a single video). */
    overlay: @Composable BoxScope.(Int?) -> Unit = {},
) {
    // Always the first video's 3 frames, for a pack too.
    val frames = rememberVideoPreviews(entry, firstPreviewVideo(entry)?.index)
    Box(modifier) {
        VideoPreviewPager(entry, frames, Modifier.fillMaxSize(), large)
        overlay(null)
    }
}

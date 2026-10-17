package webtor.app

import android.graphics.Bitmap
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
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
                    current.paused || current.engineId == null) break
                delay(4_000L)
            }
        }
    }
    return frames
}

/** A pack is paged by episode, not three indistinguishable stills from episode one. */
@Composable
fun EpisodeVideoPreview(entry: DownloadEntry, modifier: Modifier = Modifier, large: Boolean = false) {
    val videos = remember(entry.files, entry.selected) { previewVideos(entry) }
    if (videos.size <= 1) {
        val frames = rememberVideoPreviews(entry)
        VideoPreviewPager(entry, frames, modifier, large)
        return
    }
    val repository = (LocalContext.current.applicationContext as WebtorApp).thumbnails
    val pager = rememberPagerState(
        initialPage = videos.indexOfFirst { it.index == entry.focusedFile }.coerceAtLeast(0),
        pageCount = { videos.size },
    )
    LaunchedEffect(entry.key, entry.focusedFile) {
        val focused = videos.indexOfFirst { it.index == entry.focusedFile }
        if (focused >= 0) pager.scrollToPage(focused)
    }
    Box(modifier) {
        HorizontalPager(state = pager, key = { videos[it].index }, modifier = Modifier.fillMaxSize()) { page ->
            val video = videos[page]
            // Only the settled episode starts decoding; swiping through a large pack
            // does not queue a native preview for every offscreen episode.
            val frames = if (page == pager.settledPage) rememberVideoPreviews(entry, video.index)
                else repository.cached(entry, video.index)
            val frame = frames?.let { it.getOrNull(if (entry.directPlay) it.lastIndex else it.size / 2) }
            VideoPreviewPager(entry.copy(title = video.name), frame?.let { listOf(it) }, Modifier.fillMaxSize(), large)
        }
        val current = videos[pager.currentPage.coerceIn(videos.indices)]
        Text(
            "${pager.currentPage + 1}/${videos.size} · ${current.name}",
            modifier = Modifier.align(Alignment.BottomCenter).fillMaxWidth()
                .background(Color.Black.copy(alpha = 0.65f), RoundedCornerShape(bottomStart = 8.dp, bottomEnd = 8.dp))
                .padding(horizontal = 6.dp, vertical = 4.dp),
            color = Color.White,
            style = MaterialTheme.typography.labelSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

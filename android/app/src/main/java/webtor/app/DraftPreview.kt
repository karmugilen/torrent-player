package webtor.app

import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.delay

/** Drafts share the eventual library identity so their first real frame survives Add. */
@Composable
fun DraftVideoPreview(draft: PrepareDraft) {
    val torrent = draft.torrent ?: return
    if (!torrent.ready || draft.committed) return
    val entry = DownloadEntry(
        key = torrent.infoHash ?: draft.engineId,
        title = torrent.name.orEmpty(), source = draft.source, metadata = "",
        engineId = draft.engineId, destination = "", selected = draft.selected,
        files = torrent.files.map { SavedFile(it.index, it.name, it.path, it.length,
            progress = it.progress, verifiedBytes = it.verifiedBytes) },
        directPlay = true,
    )
    if (firstPreviewVideo(entry) == null) return
    val repository = (LocalContext.current.applicationContext as WebtorApp).thumbnails
    var frames by remember(entry.key) { mutableStateOf(repository.cached(entry)) }
    val current by rememberUpdatedState(entry)
    LaunchedEffect(draft.engineId, draft.selected) {
        while (true) {
            val next = repository.previews(current)
            if (next.isNotEmpty()) { frames = next; break }
            delay(4_000L)
        }
    }
    ThemedCard(Modifier.fillMaxWidth()) {
        VideoPreviewPager(entry, frames, Modifier.fillMaxWidth().aspectRatio(16f / 9f))
    }
}

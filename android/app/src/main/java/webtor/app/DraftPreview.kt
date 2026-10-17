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
    ThemedCard(Modifier.fillMaxWidth()) {
        EpisodeVideoPreview(entry, Modifier.fillMaxWidth().aspectRatio(16f / 9f))
    }
}

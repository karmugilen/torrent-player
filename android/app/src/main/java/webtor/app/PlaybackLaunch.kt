package webtor.app

/** Content-provider grants travel with the playlist, including sidecar subtitles. */
data class PlaybackItem(val uri: String, val name: String)

internal fun naturalMediaKey(path: String): String = Regex("\\d+").replace(path.lowercase(java.util.Locale.ROOT)) {
    it.value.trimStart('0').ifEmpty { "0" }.padStart(20, '0')
}

data class PlaybackFiles(val videos: List<SavedFile>, val subtitles: List<SavedFile>)

fun playbackFiles(entry: DownloadEntry, file: SavedFile): PlaybackFiles {
    val playable = entry.files.filter {
        (it.index in entry.selected || (!entry.directPlay && it.isVerifiedComplete)) && (entry.directPlay || it.uri != null)
    }
    val playlist = playable.filter { it.name.isVideoName() }.sortedBy { naturalMediaKey(it.path) }
    val stem = file.path.substringBeforeLast('.').lowercase(java.util.Locale.ROOT)
    val subtitles = playable.filter {
        val path = it.path.lowercase(java.util.Locale.ROOT)
        path.substringAfterLast('.', "") in setOf("srt", "ass", "ssa", "vtt", "sub") &&
            (path.substringBeforeLast('.') == stem || path.startsWith("$stem."))
    }
    return PlaybackFiles(playlist, subtitles)
}

fun playbackEvent(entry: DownloadEntry, file: SavedFile, positionMs: Long = file.playbackPositionMs): UiEvent.OpenContent {
    fun item(f: SavedFile) = PlaybackItem(PlaybackProvider.uriFor(entry, f).toString(), f.name)
    val files = playbackFiles(entry,file)
    return UiEvent.OpenContent(item(file).uri, mimeFor(file.name), file.name,
        files.videos.map(::item), files.subtitles.map(::item), positionMs)
}

fun playbackTime(positionMs: Long): String {
    val seconds = positionMs.coerceAtLeast(0L)/1000
    return if (seconds >= 3600) "%d:%02d:%02d".format(java.util.Locale.ROOT, seconds/3600, seconds/60%60, seconds%60)
    else "%d:%02d".format(java.util.Locale.ROOT,seconds/60,seconds%60)
}

fun currentResumePosition(entry: DownloadEntry): Long = entry.files.find { it.index == pickPlayIndex(entry) }?.playbackPositionMs ?: 0L

fun returnedPlaybackUri(returned: String?, launched: String?, playlist: Set<String>): String? = when {
    returned != null -> returned.takeIf { it == launched || it in playlist }
    playlist.size <= 1 -> launched
    else -> null // A player may have advanced episodes without identifying the returned item.
}

fun SavedFile.withPlaybackPosition(positionMs: Long, durationMs: Long): SavedFile = copy(
    playbackPositionMs = if (durationMs > 0 && positionMs >= durationMs-3000) 0L
        else positionMs.coerceAtLeast(0L).let { if (durationMs > 0) it.coerceAtMost(durationMs) else it },
    playbackDurationMs = durationMs.coerceAtLeast(0L),
)

/** Approximate prefetch hint only; the player's timestamp controls its seek. */
fun SavedFile.resumeByteHint(): Long? {
    if (length <= 0 || playbackPositionMs < 0 || playbackDurationMs <= 0) return null
    val fraction = (playbackPositionMs.toDouble() / playbackDurationMs).coerceIn(0.0, 1.0)
    return (length.toDouble() * fraction).toLong().coerceIn(0L, length - 1)
}

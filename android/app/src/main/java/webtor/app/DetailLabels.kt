package webtor.app

internal data class LeftOffChoice(val file: SavedFile, val qualifyingCount: Int)

internal enum class DetailFileBorder { NEUTRAL, PRIORITY, LEFT_OFF }

/** Selected or verified videos with a resume point that is not in the last 3 seconds. */
internal fun isLeftOffCandidate(file: SavedFile, selected: Boolean): Boolean {
    if (!file.name.isVideoName() && !file.path.isVideoName()) return false
    if (!selected && !file.isVerifiedComplete) return false
    return showsLeftOffTime(file.playbackPositionMs, file.playbackDurationMs)
}

/**
 * True when a position is worth showing.
 * Matches the 3s end window in [SavedFile.withPlaybackPosition], which clears a finished watch.
 */
internal fun showsLeftOffTime(positionMs: Long, durationMs: Long): Boolean {
    if (positionMs <= 0L) return false
    if (durationMs > 0L && positionMs >= durationMs - 3_000L) return false
    return true
}

/** Bar only accompanies a visible left-off time, and only when duration is known. */
internal fun showsPlaybackBar(positionMs: Long, durationMs: Long): Boolean =
    showsLeftOffTime(positionMs, durationMs) && durationMs > 0L

internal fun playbackFraction(positionMs: Long, durationMs: Long): Float? {
    if (durationMs <= 0L) return null
    if (positionMs <= 0L) return 0f
    return (positionMs.toDouble() / durationMs.toDouble()).toFloat().coerceIn(0f, 1f)
}

internal fun leftOffLine(positionMs: Long): String = "Left off at ${playbackTime(positionMs)}"

/** One summary line. Several qualifying videos also name the chosen file. */
internal fun leftOffSummaryLine(file: SavedFile, qualifyingCount: Int): String {
    val time = leftOffLine(file.playbackPositionMs)
    if (qualifyingCount <= 1 || file.name.isBlank()) return time
    return "$time · ${file.name}"
}

/** Newest [SavedFile.bookmarkUpdatedAt], then the lower file index. */
internal fun leftOffChoice(files: List<SavedFile>, selected: Set<Int>): LeftOffChoice? {
    var best: SavedFile? = null
    var count = 0
    for (file in files) {
        if (!isLeftOffCandidate(file, file.index in selected)) continue
        count++
        val current = best
        if (current == null ||
            file.bookmarkUpdatedAt > current.bookmarkUpdatedAt ||
            (file.bookmarkUpdatedAt == current.bookmarkUpdatedAt && file.index < current.index)
        ) {
            best = file
        }
    }
    val chosen = best ?: return null
    return LeftOffChoice(chosen, count)
}

internal fun detailTypeLabel(name: String, path: String): String = when {
    name.isVideoName() || path.isVideoName() -> "Video"
    name.isSubtitleName() || path.isSubtitleName() -> "Subtitle"
    name.isAudioName() || path.isAudioName() -> "Audio"
    else -> "Other"
}

/** Parent directory when path is nested and is not itself the display name. */
internal fun parentFolderLabel(name: String, path: String): String? {
    if (!path.contains('/') || path == name) return null
    val parent = path.substringBeforeLast('/')
    return parent.ifEmpty { null }
}

/** Priority download keeps the primary border when the row is also the left-off file. */
internal fun detailFileBorder(priorityDownload: Boolean, comeBack: Boolean): DetailFileBorder = when {
    priorityDownload -> DetailFileBorder.PRIORITY
    comeBack -> DetailFileBorder.LEFT_OFF
    else -> DetailFileBorder.NEUTRAL
}

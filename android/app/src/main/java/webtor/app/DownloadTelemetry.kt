package webtor.app

/** A status from an earlier native record must never supply a restored row's rate. */
fun DownloadEntry.liveStatus() = status?.takeIf { engineId != null && it.id == engineId }

fun DownloadEntry.hasActiveTransfer(): Boolean =
    engineId != null && !paused && !complete && !controlsBusy() && error == null &&
        lifecycleState !in setOf(EntryLifecycleState.PAUSED, EntryLifecycleState.STOPPED, EntryLifecycleState.ERROR)

fun DownloadEntry.liveDownloadSpeed(): Long? =
    if (hasActiveTransfer() && !checking) liveStatus()?.downloadSpeed?.coerceAtLeast(0L) else null

fun DownloadEntry.liveUploadSpeed(): Long? =
    if ((hasActiveTransfer() || liveStatus()?.seeding == true) && !paused && !checking) liveStatus()?.uploadSpeed?.coerceAtLeast(0L) else null

fun DownloadEntry.selectedEta(): Long? {
    if (directPlay) return null
    val speed = liveDownloadSpeed() ?: return null
    return if (speed <= 0 || downloaded >= total) null
    else ((total - downloaded).toDouble() / speed * 1000).toLong()
}

fun transferTelemetry(entry: DownloadEntry): String? {
    if (entry.liveStatus()?.seeding == true) return "Seeding · ↑ ${formatSpeed(entry.liveStatus()?.uploadSpeed ?: 0L)} · ratio ${"%.2f".format(entry.liveStatus()?.seedRatio ?: 0.0)}"
    if (!entry.hasActiveTransfer() || entry.checking) return null
    return buildList {
        add(entry.liveDownloadSpeed()?.let(::formatSpeed) ?: "—")
        entry.liveStatus()?.let { add(formatPeers(it.numPeers.coerceAtLeast(0))) }
        formatEta(entry.selectedEta())?.let(::add)
    }.joinToString(" · ")
}

fun entryTelemetry(entry: DownloadEntry): String = buildString {
    append(when {
        entry.directPlay -> "Direct Play · ${formatBytes(entry.downloaded)} cached"
        entry.metadataReady -> "${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)}"
        else -> entry.stateLabel()
    })
    if (entry.checking) append(" · Checked ${entry.checkPercent}%")
    else transferTelemetry(entry)?.let { append(" · $it") }
}

data class DownloadNotification(val title: String, val text: String, val progress: Int, val multiple: Boolean, val playbackOnly: Boolean = false)

fun downloadNotification(entries: List<DownloadEntry>, playbackKeys: Set<String> = emptySet()): DownloadNotification? {
    val active = entries.filter { it.hasActiveTransfer() }
    if (active.isEmpty()) {
        val seeds = entries.filter { !it.paused && !it.isDeleting && it.liveStatus()?.seeding == true }
        if (seeds.isNotEmpty()) return DownloadNotification(
            title = if (seeds.size == 1) seeds.first().title else "${seeds.size} torrents seeding",
            text = "Seeding · ↑ ${formatSpeed(seeds.sumOf { it.liveStatus()?.uploadSpeed ?: 0L })}", progress = 100, multiple = seeds.size > 1)
        val playing = entries.filter { it.key in playbackKeys && !it.isDeleting }
        return if (playing.isEmpty()) null else DownloadNotification(
            title = if (playing.size == 1) playing.first().title else "Video playback",
            text = "Serving video to your player", progress = 100, multiple = playing.size > 1, playbackOnly = true)
    }
    val streaming = active.filter { it.directPlay && !it.checking }
    val downloading = active.filter { !it.directPlay && it.metadataReady && !it.checking }
    val checking = active.filter { it.checking }
    val waiting = active.count { !it.metadataReady && !it.checking }
    val total = downloading.sumOf { it.total }
    val got = downloading.sumOf { it.downloaded }
    val progress = if (total <= 0) 0 else (100.0 * got / total).toInt().coerceIn(0, 100)
    val text = when {
        streaming.isNotEmpty() && downloading.isEmpty() -> buildString {
            append("Direct Play · ↓ " + formatSpeed(streaming.sumOf { it.liveDownloadSpeed() ?: 0L }))
            val statuses = streaming.mapNotNull { it.liveStatus() }
            if (statuses.size == streaming.size) append(" · ${formatPeers(statuses.sumOf { it.numPeers.coerceAtLeast(0) })}")
        }
        downloading.isNotEmpty() -> buildString {
            append("${formatBytes(got)} / ${formatBytes(total)} · ")
            val speeds = downloading.mapNotNull { it.liveDownloadSpeed() }
            append(if (speeds.isEmpty()) "—" else formatSpeed(speeds.sum()))
            val statuses = downloading.mapNotNull { it.liveStatus() }
            // A partial snapshot cannot claim a total peer count.
            if (statuses.size == downloading.size) append(" · ${formatPeers(statuses.sumOf { it.numPeers.coerceAtLeast(0) })}")
            if (streaming.isNotEmpty()) append(" · ${streaming.size} playing")
            if (waiting > 0) append(" · $waiting waiting")
        }
        checking.isNotEmpty() -> buildString {
            append("Checking saved data · ${checking.map { it.checkPercent }.average().toInt()}%")
            if (streaming.isNotEmpty()) append(" · ${streaming.size} playing")
            if (waiting > 0) append(" · $waiting waiting")
        }
        else -> "Waiting for peers"
    }
    return DownloadNotification(
        title = if (active.size == 1) active.first().title else "${active.size} downloads",
        text = text,
        progress = progress,
        multiple = active.size > 1,
    )
}

/** One library status line, or null when nothing is transferring or blocked. */
fun libraryActivityLine(entries: List<DownloadEntry>): String? {
    val downloads = entries.count { !it.directPlay && it.hasActiveTransfer() }
    val directPlays = entries.count { it.directPlay && it.hasActiveTransfer() }
    val attention = entries.count { it.matchesFilter(LibraryFilter.NEEDS_ATTENTION) }
    val speed = entries.sumOf { entry ->
        val live = entry.liveDownloadSpeed()
        if (live != null && live > 0L) live else 0L
    }
    val parts = listOfNotNull(
        activityCount(downloads, "download", "downloads"),
        activityCount(directPlays, "direct play", "direct plays"),
        if (speed > 0L) formatSpeed(speed) else null,
        activityCount(attention, "needs attention", "need attention"),
    )
    return parts.takeIf { it.isNotEmpty() }?.joinToString(" · ")
}

private fun activityCount(count: Int, singular: String, plural: String): String? = when (count) {
    0 -> null
    1 -> "1 $singular"
    else -> "$count $plural"
}

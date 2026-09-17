package webtor.app

/** A status from an earlier native record must never supply a restored row's rate. */
fun DownloadEntry.liveStatus() = status?.takeIf { engineId != null && it.id == engineId }

fun DownloadEntry.hasActiveTransfer(): Boolean =
    engineId != null && !paused && !complete && !controlsBusy() && error == null &&
        lifecycleState !in setOf(EntryLifecycleState.PAUSED, EntryLifecycleState.STOPPED, EntryLifecycleState.ERROR)

fun DownloadEntry.liveDownloadSpeed(): Long? =
    if (hasActiveTransfer() && !checking) liveStatus()?.downloadSpeed?.coerceAtLeast(0L) else null

fun DownloadEntry.liveUploadSpeed(): Long? =
    if (hasActiveTransfer() && !checking) liveStatus()?.uploadSpeed?.coerceAtLeast(0L) else null

fun DownloadEntry.selectedEta(): Long? {
    val speed = liveDownloadSpeed() ?: return null
    return if (speed <= 0 || downloaded >= total) null
    else ((total - downloaded).toDouble() / speed * 1000).toLong()
}

fun transferTelemetry(entry: DownloadEntry): String? {
    if (!entry.hasActiveTransfer() || entry.checking) return null
    return buildList {
        add(entry.liveDownloadSpeed()?.let(::formatSpeed) ?: "—")
        entry.liveStatus()?.let { add(formatPeers(it.numPeers.coerceAtLeast(0))) }
        formatEta(entry.selectedEta())?.let(::add)
    }.joinToString(" · ")
}

fun entryTelemetry(entry: DownloadEntry): String = buildString {
    append(if (entry.metadataReady) "${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)}" else entry.stateLabel())
    if (entry.checking) append(" · Checked ${entry.checkPercent}%")
    else transferTelemetry(entry)?.let { append(" · $it") }
}

data class DownloadNotification(val title: String, val text: String, val progress: Int, val multiple: Boolean)

fun downloadNotification(entries: List<DownloadEntry>): DownloadNotification? {
    val active = entries.filter { it.hasActiveTransfer() }
    if (active.isEmpty()) return null
    val downloading = active.filter { it.metadataReady && !it.checking }
    val checking = active.filter { it.checking }
    val waiting = active.count { !it.metadataReady && !it.checking }
    val total = downloading.sumOf { it.total }
    val got = downloading.sumOf { it.downloaded }
    val progress = if (total <= 0) 0 else (100.0 * got / total).toInt().coerceIn(0, 100)
    val text = when {
        downloading.isNotEmpty() -> buildString {
            append("${formatBytes(got)} / ${formatBytes(total)} · ")
            val speeds = downloading.mapNotNull { it.liveDownloadSpeed() }
            append(if (speeds.isEmpty()) "—" else formatSpeed(speeds.sum()))
            val statuses = downloading.mapNotNull { it.liveStatus() }
            // A partial snapshot cannot claim a total peer count.
            if (statuses.size == downloading.size) append(" · ${formatPeers(statuses.sumOf { it.numPeers.coerceAtLeast(0) })}")
            if (waiting > 0) append(" · $waiting waiting")
        }
        checking.isNotEmpty() -> buildString {
            append("Checking saved data · ${checking.map { it.checkPercent }.average().toInt()}%")
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

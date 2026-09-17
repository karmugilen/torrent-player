package webtor.app

import android.webkit.MimeTypeMap
import java.util.Locale
import webtor.core.TorrentStatus

private val VIDEO_EXT = Regex("\\.(mp4|m4v|mkv|webm|mov|avi)$", RegexOption.IGNORE_CASE)
private val AUDIO_EXT = Regex("\\.(mp3|m4a|aac|flac|ogg|wav|opus)$", RegexOption.IGNORE_CASE)

fun String.isVideoName(): Boolean = VIDEO_EXT.containsMatchIn(this)
fun String.isAudioName(): Boolean = AUDIO_EXT.containsMatchIn(this)

fun formatBytes(n: Long): String {
    if (n < 1024) return "$n B"
    val kb = n / 1024.0
    if (kb < 1024) return String.format(Locale.US, "%.1f KB", kb)
    val mb = kb / 1024.0
    if (mb < 1024) return String.format(Locale.US, "%.1f MB", mb)
    return String.format(Locale.US, "%.2f GB", mb / 1024.0)
}

fun formatSpeed(bytesPerSec: Long): String = "${formatBytes(bytesPerSec)}/s"

fun formatEta(ms: Long?): String? {
    if (ms == null || ms < 0 || ms > 1000L * 60 * 60 * 24 * 40) return null
    val sec = (ms / 1000).coerceAtLeast(0)
    val h = sec / 3600
    val m = (sec % 3600) / 60
    return when {
        h >= 48 -> "${h / 24} days left"
        h > 0 -> "${h}h ${m}m left"
        m > 0 -> "$m min left"
        else -> "less than a minute"
    }
}

fun formatPeers(n: Int): String = if (n == 1) "1 peer" else "$n peers"

/** Show tracker identity without leaking paths, query tokens, or announce keys. */
fun redactedTrackerUrl(value: String): String {
    // Parse only the scheme, host and numeric port. This deliberately drops
    // announce paths and query parameters, which may contain private tokens.
    val match = Regex("^([a-z][a-z0-9+.-]*):\\/\\/([^/?#:]+)(?::([0-9]{1,5}))?").find(value.trim().lowercase(Locale.US))
        ?: return "tracker"
    val scheme = match.groupValues[1]
    val host = match.groupValues[2]
    val port = match.groupValues[3].takeIf { it.isNotBlank() }?.let { ":$it" }.orEmpty()
    return "$scheme://$host$port"
}

fun mediaType(entry: DownloadEntry): String {
    val names = entry.files.filter { it.index in entry.selected }.map { it.name }
    if (names.any { it.isVideoName() }) return "Video"
    if (names.any { it.isAudioName() }) return "Audio"
    return "Files"
}

fun mimeFor(name: String): String =
    commonMediaMime(name) ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
        ?: "application/octet-stream"

internal fun commonMediaMime(name: String): String? = when (name.substringAfterLast('.', "").lowercase()) {
    "mp4", "m4v" -> "video/mp4"
    "mkv" -> "video/x-matroska"
    "webm" -> "video/webm"
    "avi" -> "video/x-msvideo"
    "mov" -> "video/quicktime"
    "ts", "m2ts", "mts" -> "video/mp2t"
    "mp3" -> "audio/mpeg"
    "m4a", "m4b" -> "audio/mp4"
    "flac" -> "audio/flac"
    "ogg", "opus" -> "audio/ogg"
    "wav" -> "audio/wav"
    else -> null
}

fun TorrentStatus.timeRemainingMs(): Long? {
    timeRemaining?.takeIf { it >= 0 }?.let { return it }
    if (downloadSpeed > 0 && length > downloaded) {
        return ((length - downloaded).toDouble() / downloadSpeed * 1000.0).toLong()
    }
    return null
}

fun defaultVideoSelection(files: List<webtor.core.TorrentFile>): Set<Int> {
    val videos = files.filter { it.name.isVideoName() }.map { it.index }.toSet()
    if (videos.isNotEmpty()) return videos
    return files.map { it.index }.toSet()
}

fun pickPlayIndex(entry: DownloadEntry): Int? {
    // Prefer active transfer files; if all files were deselected, keep a
    // completed local file discoverable from the library.
    val files = entry.files.filter { it.index in entry.selected }.ifEmpty {
        entry.files.filter { it.uri != null && it.isVerifiedComplete }
    }
    val videos = files.filter { it.name.isVideoName() }
    return (videos.ifEmpty { files }).maxByOrNull { it.length }?.index
}

fun DownloadEntry.canPlay(): Boolean =
    !isDeleting && !controlsBusy() && files.any { !it.uri.isNullOrBlank() && (it.index in selected || it.isVerifiedComplete) }

fun DownloadEntry.selectedVideoCount(): Int =
    files.count { it.index in selected && it.name.isVideoName() }

/** First selected video by index (episode 1 of a pack), not the most-downloaded file. */
fun firstPreviewVideo(entry: DownloadEntry): SavedFile? =
    entry.files.filter { it.index in entry.selected && it.name.isVideoName() }.minByOrNull { it.index }

/** Complete: 20/50/80 of full duration. Live: up to 3 spots inside downloaded span. */
fun previewRatios(progress: Double, complete: Boolean): List<Double> {
    if (complete || progress >= 1.0) return listOf(0.20, 0.50, 0.80)
    return livePreviewRatios(progress)
}

/**
 * Up to three seek ratios inside bytes already on disk.
 * Does not request peer seeks or change download priority.
 */
fun livePreviewRatios(progress: Double): List<Double> {
    val max = progress.coerceIn(0.0, 1.0)
    if (max < 0.03) return emptyList()
    if (max < 0.10) return listOf((max * 0.5).coerceAtLeast(0.02))
    if (max < 0.25) {
        return listOf(max * 0.35, max * 0.75)
            .map { it.coerceIn(0.02, max) }
            .distinct()
    }
    return listOf(max * 0.20, max * 0.50, max * 0.80)
        .map { it.coerceIn(0.02, max) }
        .distinct()
}

/** Midpoint of the live span; kept for callers/tests that want one ratio. */
fun livePreviewRatio(progress: Double): Double {
    val ratios = livePreviewRatios(progress)
    if (ratios.isEmpty()) return (progress.coerceIn(0.02, 1.0) * 0.85).coerceIn(0.02, 1.0)
    return ratios[ratios.size / 2]
}

/** Latest ratio a seek may use: full file when complete, otherwise the downloaded span. */
fun previewMaxRatio(progress: Double, complete: Boolean): Double =
    if (complete || progress >= 1.0) 1.0 else progress.coerceIn(0.02, 1.0)

/** Kept for tests/helpers; complete thumbs no longer walk nudge slots. */
fun previewSlotRatios(target: Double, progress: Double, complete: Boolean): List<Double> {
    val maxRatio = previewMaxRatio(progress, complete)
    return listOf(target.coerceIn(0.0, maxRatio))
}

/** True when a 16×16 ARGB sample's average Rec.601 luma is below 18 (black title cards). */
fun tooDarkLuma(pixels: IntArray): Boolean {
    if (pixels.isEmpty()) return true
    var sum = 0L
    for (p in pixels) {
        val r = (p ushr 16) and 0xFF
        val g = (p ushr 8) and 0xFF
        val b = p and 0xFF
        sum += (r * 299L + g * 587L + b * 114L) / 1000L
    }
    return sum / pixels.size < 18
}

/** 5% steps (0..19) while downloading; 20 once complete. */
fun previewUpdateBucket(progress: Double, complete: Boolean): Int =
    if (complete || progress >= 1.0) 20 else (progress * 20).toInt().coerceIn(0, 19)

fun infoHashFromMagnet(value: String): String? {
    val match = Regex("xt=urn:btih:([a-zA-Z0-9]+)", RegexOption.IGNORE_CASE).find(value) ?: return null
    return match.groupValues[1].lowercase()
}

fun looksLikeTorrentSource(value: String): Boolean = supportedTorrentLink(value) != null

fun prefetchStatus(draft: PrepareDraft?): String? {
    if (draft == null) return null
    val t = draft.torrent
    val peers = t?.numPeers ?: 0
    return when {
        t?.ready == true -> "Files ready · ${formatPeers(peers)}"
        peers > 0 -> "Connecting · ${formatPeers(peers)}"
        else -> "Finding peers…"
    }
}

fun readableError(t: Throwable): String = readableError(t.message)

fun readableError(message: String?): String {
    val msg = message?.trim().orEmpty()
    if (msg.isEmpty()) return "Something went wrong. Try again."
    val lower = msg.lowercase()
    return when {
        "failed to connect" in lower || "connection refused" in lower || "engine did not start" in lower || "engine down" in lower ->
            "The download engine is not running. Close Torrent Player and open it again."
        "enospc" in lower || "no space" in lower || "not enough storage" in lower ->
            msg
        "unavailable" in lower || "cannot access this folder" in lower || "folder access" in lower || "revoked" in lower ->
            "Folder access was revoked. Choose the folder again, or delete the files from your file manager."
        "permission" in lower && "folder" in lower ->
            "Folder access was revoked. Re-grant access to the download folder."
        "no peer" in lower || "no seeds" in lower ->
            "No peers found. The torrent may have no seeds right now. Try again later."
        "timed out" in lower || "timeout" in lower || "504" in lower ->
            "Timed out waiting for torrent metadata. Check your connection or try again."
        "404" in lower || "not found" in lower ->
            "Download session was removed. Resume to restart."
        else -> msg
    }
}

// Use the same target for local playback and streaming.
fun completedPlayFile(entry: DownloadEntry, fileIndex: Int? = pickPlayIndex(entry)): SavedFile? =
    // A fully saved file remains locally playable after the user deselects it
    // from transfer demand. Incomplete files still require selection.
    entry.files.find { it.index == fileIndex && it.uri != null && it.isVerifiedComplete }

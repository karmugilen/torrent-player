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

fun mediaType(entry: DownloadEntry): String {
    val names = entry.files.filter { it.index in entry.selected }.map { it.name }
    if (names.any { it.isVideoName() }) return "Video"
    if (names.any { it.isAudioName() }) return "Audio"
    return "Files"
}

fun mimeFor(name: String): String =
    MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
        ?: "application/octet-stream"

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
    val files = entry.files.filter { it.index in entry.selected }
    val videos = files.filter { it.name.isVideoName() }
    return (videos.ifEmpty { files }).maxByOrNull { it.length }?.index
}

fun infoHashFromMagnet(value: String): String? {
    val match = Regex("xt=urn:btih:([a-zA-Z0-9]+)", RegexOption.IGNORE_CASE).find(value) ?: return null
    return match.groupValues[1].lowercase()
}

fun looksLikeTorrentSource(value: String): Boolean {
    val src = value.trim()
    if (src.startsWith("magnet:", ignoreCase = true)) {
        val hash = infoHashFromMagnet(src) ?: return false
        return hash.length == 40 || hash.length == 32
    }
    return src.startsWith("http://", ignoreCase = true) || src.startsWith("https://", ignoreCase = true)
}

fun prefetchStatus(draft: PrepareDraft?): String? {
    if (draft == null) return null
    val t = draft.torrent
    val peers = t?.numPeers ?: 0
    val got = t?.downloaded ?: 0L
    return when {
        t?.ready == true && got > 0 ->
            "${formatBytes(got)} already downloading · ${formatPeers(peers)}"
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
        else -> msg
    }
}

// Use the same target for local playback and streaming.
fun completedPlayFile(entry: DownloadEntry, fileIndex: Int? = pickPlayIndex(entry)): SavedFile? =
    entry.files.find { it.index in entry.selected && it.index == fileIndex && it.uri != null && (it.length == 0L || it.progress >= 1.0) }

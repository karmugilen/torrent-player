package webtor.app

import android.webkit.MimeTypeMap
import java.util.Locale
import webtor.core.TorrentStatus

private val VIDEO_EXT = Regex("\\.(mp4|m4v|mkv|webm|mov|avi)$", RegexOption.IGNORE_CASE)
private val AUDIO_EXT = Regex("\\.(mp3|m4a|aac|flac|ogg|wav|opus)$", RegexOption.IGNORE_CASE)
private val SUBTITLE_EXT = Regex("\\.(srt|vtt|ass|ssa|sub)$", RegexOption.IGNORE_CASE)

fun String.isVideoName(): Boolean = VIDEO_EXT.containsMatchIn(this)
fun String.isAudioName(): Boolean = AUDIO_EXT.containsMatchIn(this)
fun String.isSubtitleName(): Boolean = SUBTITLE_EXT.containsMatchIn(this)

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
    val videos = files.filter { it.name.isVideoName() }
    if (videos.isNotEmpty()) {
        val videoStems = videos.map { it.path.substringBeforeLast('.').lowercase(Locale.ROOT) }.toSet()
        val matchingSubs = files.filter { file ->
            file.name.isSubtitleName() && videoStems.any { stem ->
                val path = file.path.lowercase(Locale.ROOT)
                path == stem || path.startsWith("$stem.") || path.startsWith("$stem/")
            }
        }
        return (videos + matchingSubs).map { it.index }.toSet()
    }
    return files.map { it.index }.toSet()
}

fun pickPlayIndex(entry: DownloadEntry): Int? {
    // Prefer active transfer files; if all files were deselected, keep a
    // completed local file discoverable from the library.
    val files = entry.files.filter { it.index in entry.selected }.ifEmpty {
        entry.files.filter { it.uri != null && it.isVerifiedComplete }
    }
    val videos = files.filter { it.name.isVideoName() }
    return if (videos.size > 1) videos.minByOrNull { naturalMediaKey(it.path) }?.index
    else (videos.ifEmpty { files }).maxByOrNull { it.length }?.index
}

fun DownloadEntry.canPlay(): Boolean =
    !isDeleting && !controlsBusy() && files.any { (directPlay || !it.uri.isNullOrBlank()) && (it.index in selected || (!directPlay && it.isVerifiedComplete)) }

fun DownloadEntry.selectedVideoCount(): Int =
    files.count { it.index in selected && it.name.isVideoName() }

/** Each selected episode owns its preview identity, independent of download progress. */
fun previewVideos(entry: DownloadEntry): List<SavedFile> =
    entry.files.filter { it.index in entry.selected && (it.name.isVideoName() || it.path.isVideoName()) }
        .sortedWith(compareBy<SavedFile> { naturalMediaKey(it.path) }.thenBy { it.index })

fun firstPreviewVideo(entry: DownloadEntry): SavedFile? {
    val videos = previewVideos(entry)
    return videos.find { it.index == entry.focusedFile } ?: videos.firstOrNull()
}

/** An explicit episode must never fall back to another episode's image. */
fun previewVideo(entry: DownloadEntry, fileIndex: Int?): SavedFile? =
    if (fileIndex == null) firstPreviewVideo(entry)
    else entry.files.find {
        it.index == fileIndex && it.index in entry.selected && (it.name.isVideoName() || it.path.isVideoName())
    }

/** Compare small RGB samples; nearby seeks often decode the same key frame. */
internal fun previewFramesDiffer(first: IntArray, second: IntArray): Boolean {
    if (first.size != second.size) return true
    if (first.isEmpty()) return false
    var difference = 0L
    for (i in first.indices) {
        for (shift in 0..16 step 8) {
            difference += kotlin.math.abs(((first[i] shr shift) and 255) - ((second[i] shr shift) and 255))
        }
    }
    return difference > first.size * 3L * 6L
}

/** Decoder targets shared by saved-file and native preview reads. */
internal fun previewTargetTimesUs(
    identity: String,
    durationMs: Long,
    availableRatio: Double = 1.0,
    playbackOffsetUs: Long? = null,
): List<Long> {
    if (durationMs <= 0L) return listOf(500_000L, 1_500_000L, 3_000_000L)
    val durationUs = durationMs.coerceAtMost(Long.MAX_VALUE / 1000L) * 1000L
    if (playbackOffsetUs != null) {
        val body = playbackOffsetUs.coerceIn(0L, durationUs - 1L)
        val random = kotlin.random.Random(identity.hashCode() xor (body / 12_000_000L).toInt())
        val window = minOf(body, 6_000_000L)
        return listOf((body - (random.nextDouble() * window).toLong()), body).distinct()
    }
    val availableUs = (durationUs * availableRatio.coerceIn(0.0, 1.0)).toLong()
    if (availableUs <= 0L) return emptyList()
    // Stable jitter in separated middle bands: refresh/recomposition does not
    // cause new seeks, while different torrent/file identities avoid shared intros.
    val random = kotlin.random.Random(identity.hashCode())
    return listOf(0.18 to 0.36, 0.42 to 0.60, 0.66 to 0.84)
        .map { (from, to) -> (availableUs * random.nextDouble(from, to)).toLong() }
        .distinct()
}

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

const val LOW_BATTERY_PLAY_MESSAGE =
    "Battery is under 15% and not charging, so torrent data is paused. Charge the phone or turn off Pause on low battery in Settings."

/** Why Play must not start a torrent, or null when payload transfers are allowed. */
internal fun playbackDataBlock(batteryPaused: Boolean, networkAllowed: Boolean, networkReason: String?): String? {
    if (batteryPaused) return LOW_BATTERY_PLAY_MESSAGE
    if (!networkAllowed) return networkReason ?: "Waiting for an allowed network."
    return null
}

fun readableError(t: Throwable): String = readableError(t.message)

fun readableError(message: String?): String {
    val msg = message?.trim().orEmpty()
    if (msg.isEmpty()) return "Something went wrong. Try again."
    val lower = msg.lowercase()
    return when {
        // The engine runs inside the app; only its own start failures mean it is down.
        "engine did not start" in lower || "engine down" in lower || "native engine initialization failed" in lower ->
            "The download engine is not running. Close Torrent Player and open it again."
        "payload transfers are disabled" in lower ->
            "Torrent data is paused by low battery or the network setting. Charge the phone, or check Settings."
        isNetworkFailure(lower) ->
            "Couldn't connect on this network. It may block torrent traffic. Try mobile data or another Wi-Fi, " +
                "or turn on Anti-DPI in Settings."
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

/** Socket errors from trackers, peers or torrent downloads, not from the in-process engine. */
private fun isNetworkFailure(lower: String): Boolean = listOf(
    "failed to connect", "connection refused", "connection reset", "network is unreachable",
    "no route to host", "no such host", "i/o timeout", "dial tcp", "dial udp",
).any { it in lower }

// Use the same target for local playback and streaming.
fun completedPlayFile(entry: DownloadEntry, fileIndex: Int? = pickPlayIndex(entry)): SavedFile? =
    // A fully saved file remains locally playable after the user deselects it
    // from transfer demand. Incomplete files still require selection.
    entry.files.find { !entry.directPlay && it.index == fileIndex && it.uri != null && it.isVerifiedComplete }

/** RAM cache slider stops, 16 MiB to 1 GiB, finer at the small end. */
val RAM_CACHE_STEPS_MIB = listOf(16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024)

/** Slider stop nearest to [mib]. */
fun ramCacheStepIndex(mib: Int): Int =
    RAM_CACHE_STEPS_MIB.indices.minBy { kotlin.math.abs(RAM_CACHE_STEPS_MIB[it] - mib) }

fun formatRamCacheMiB(mib: Int): String = if (mib >= 1024 && mib % 1024 == 0) "${mib / 1024} GiB" else "$mib MiB"

/**
 * Extra lines for the expanded notification while a video streams: the
 * playing file's progress and upload, and how quickly seeks start playing.
 * Empty until there is something to show.
 */
internal fun playbackStatsLine(ui: UiState): String {
    val parts = mutableListOf<String>()
    val streaming = ui.library.filter { it.directPlay && it.hasActiveTransfer() && !it.checking }
    streaming.singleOrNull()?.let { entry ->
        val file = entry.files.find { it.index == (entry.status?.focusedFile ?: entry.focusedFile) }
        if (file != null && file.length > 0) {
            val got = (file.length * file.progress.coerceIn(0.0, 1.0)).toLong()
            parts += "File ${(100 * file.progress.coerceIn(0.0, 1.0)).toInt()}% · ${formatBytes(got)} of ${formatBytes(file.length)}" +
                (entry.liveStatus()?.uploadSpeed?.takeIf { it > 0 }?.let { " · ↑ ${formatSpeed(it)}" } ?: "")
        }
    }
    val seek = ui.seekStats
    if (seek != null && seek.count > 0) {
        val stalls = if (seek.stalls == 1) "1 stall" else "${seek.stalls} stalls"
        parts += "Seeks ${seek.count} · usually ${formatMs(seek.medianMs)} · last ${formatMs(seek.lastMs)} · $stalls"
    }
    return parts.joinToString("\n")
}

/** 640 ms, 1.3 s. */
internal fun formatMs(ms: Long): String = if (ms < 1000) "$ms ms" else String.format(Locale.US, "%.1f s", ms / 1000.0)

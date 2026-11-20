package webtor.app

import android.content.Context

// ---- Torrents with many videos (series) ----

private val seasonEpisode = Regex("""(?i)(?<![a-z0-9])S(\d{1,2})[ ._-]?E(\d{1,3})(?:[ ._-]?-?E(\d{1,3}))?(?![0-9])""")
private val crossEpisode = Regex("""(?i)(?<![a-z0-9])(\d{1,2})x(\d{2,3})(?![0-9a-z])""")
private val releaseTag = Regex(
    """(?i)(?<![a-z0-9])(2160p|1080p|720p|576p|480p|4k|uhd|hdr10?|web[ -]?dl|webrip|web|blu[ -]?ray|brrip|bdrip|hdtv|dvdrip|""" +
        """x26[45]|h[ ]?26[45]|hevc|avc|aac|ac3|e-?ac3|dts|ddp?[ ]?[257][ ]?[01]|10bit|8bit|remux|proper|repack|amzn|nf|dsnp|hmax|atvp)(?![a-z0-9])""",
)

private fun tidy(s: String): String = s.replace(Regex("""[\[\](){}]"""), " ")
    .replace(Regex("""\s+"""), " ").trim().trim('-', '–', '·', ' ')

/**
 * Short episode name from a file name: "Show.Name.S01E03.1080p.WEB-DL.mkv" becomes
 * "S01E03 · Show Name". Without an episode number the quality and group tags are cut;
 * when nothing sensible is left, the file name without its extension.
 */
fun episodeTitle(fileName: String): String {
    val base = fileName.substringAfterLast('/')
    val stem = if (base.isVideoName()) base.substringBeforeLast('.') else base
    val spaced = stem.replace(Regex("[._]+"), " ")
        .replace(Regex("""^\s*\[[^\]]*]\s*"""), "") // leading [Group]
        .trim()
    fun code(season: String, episode: String, last: String?) =
        "S%02dE%02d".format(season.toInt(), episode.toInt()) + (last?.takeIf { it.isNotEmpty() }?.let { "-E%02d".format(it.toInt()) } ?: "")
    seasonEpisode.find(spaced)?.let { m ->
        val show = tidy(spaced.substring(0, m.range.first))
        val c = code(m.groupValues[1], m.groupValues[2], m.groupValues[3])
        return if (show.isBlank()) c else "$c · $show"
    }
    crossEpisode.find(spaced)?.let { m ->
        val show = tidy(spaced.substring(0, m.range.first))
        val c = code(m.groupValues[1], m.groupValues[2], null)
        return if (show.isBlank()) c else "$c · $show"
    }
    val cut = releaseTag.find(spaced)?.range?.first ?: spaced.length
    return tidy(spaced.substring(0, cut)).ifBlank { stem.ifBlank { base } }
}

private val extraWords = Regex(
    """(?i)(?<![a-z])(sample|trailer|teaser|featurettes?|extras|bonus|behind[ ._-]the[ ._-]scenes|deleted[ ._-]scenes?|making[ ._-]of|interviews?)(?![a-z])""",
)

/** Samples, trailers and featurettes: playable from the file list, never part of the episode run. */
fun isExtraVideo(path: String): Boolean = extraWords.containsMatchIn(path)

private fun SavedFile.isVideo() = name.isVideoName() || path.isVideoName()
private fun SavedFile.finished() = playbackPositionMs <= 0L && playbackDurationMs > 0L && bookmarkUpdatedAt > 0L

/**
 * Continue for a series: the last watched episode at its place; when that one was watched
 * to the end, the episode after it. Null when nothing was watched yet (or all is done).
 */
fun seriesContinueIndex(entry: DownloadEntry): Int? {
    val playable = entry.files.filter {
        it.isVideo() && !isExtraVideo(it.path) && (it.index in entry.selected || it.isVerifiedComplete) &&
            (entry.directPlay || !it.uri.isNullOrBlank())
    }
    val last = playable.filter { it.bookmarkUpdatedAt > 0L }.maxByOrNull { it.bookmarkUpdatedAt } ?: return null
    if (!last.finished()) return last.index
    val order = compareBy<SavedFile> { naturalMediaKey(it.path) }.thenBy { it.index }
    return playable.filter { order.compare(last, it) < 0 }.minWithOrNull(order)?.index
}

/** What a card tap or Play opens: [seriesContinueIndex], else the usual first pick. */
fun seriesResumeIndex(entry: DownloadEntry): Int? =
    seriesContinueIndex(entry) ?: entry.resumeFile()?.index ?: pickPlayIndex(entry)

/**
 * Audio and subtitle choice, kept per torrent so the next episode (and the next time the
 * torrent is opened) uses the same language and track. Subtitles "Off" is kept as well.
 */
object TrackPrefs {
    data class Pref(val off: Boolean, val lang: String?, val title: String?, val external: Boolean)

    private fun prefs(context: Context) = context.getSharedPreferences("trackPrefs", Context.MODE_PRIVATE)

    fun load(context: Context, torrent: String, type: String): Pref? {
        val raw = prefs(context).getString("$torrent:$type", null) ?: return null
        if (raw == "off") return Pref(true, null, null, false)
        val parts = raw.split('\u0001')
        return Pref(false, parts.getOrNull(0)?.ifEmpty { null }, parts.getOrNull(1)?.ifEmpty { null }, parts.getOrNull(2) == "1")
    }

    fun save(context: Context, torrent: String, type: String, pref: Pref) {
        val raw = if (pref.off) "off" else listOf(pref.lang.orEmpty(), pref.title.orEmpty(), if (pref.external) "1" else "0").joinToString("\u0001")
        prefs(context).edit().putString("$torrent:$type", raw).apply()
    }
}

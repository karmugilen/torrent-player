package webtor.app

import webtor.core.PieceBucket
import webtor.core.TorrentFile

/**
 * Answers "is time t of the playing file here?" and "which bytes to fetch for t".
 *
 * Owns the whole path from a time in the video to the torrent: which engine stream
 * the player reads, where the file sits among the torrent's pieces, the piece map
 * as stretches of the file, the ready rule, and the prefetch window. The player
 * fetches status and pieces from the engine and passes them in ([file], [pieces]);
 * nothing in here does I/O, so every rule has a unit test (SeekReadinessTest).
 *
 * Ready rule today: a stretch counts as ready when 99% of its pieces are verified.
 * Playback itself reads blocks as they arrive (docs/DIRECT_PLAY.md), so this rule
 * is stricter than the player needs; change it here only.
 *
 * Written from the engine-polling thread, read from the UI thread.
 */
class SeekReadiness {

    /** One stretch of the file with the share of verified pieces in it, positions 0..1. */
    data class Span(val from: Float, val to: Float, val done: Float)

    /** Bytes to ask the engine for: [offset] may be below 0, the engine clamps it. */
    data class Aim(val id: String, val fileIndex: Int, val offset: Long, val length: Long)

    private data class Playing(val id: String, val index: Int, val offset: Long, val length: Long)

    @Volatile private var playing: Playing? = null
    @Volatile var spans: List<Span> = emptyList(); private set

    /** Another file starts, or the stream is not from the engine: forget everything. */
    fun clear() { playing = null; spans = emptyList() }

    /**
     * The engine's file list for torrent [id], playing file [index]. Returns true
     * when the file is partly downloaded and the piece map should come next ([pieces]).
     */
    fun file(id: String, index: Int, files: List<TorrentFile>): Boolean {
        val ordered = files.sortedBy { it.index }
        val file = ordered.find { it.index == index } ?: return false
        if (file.length <= 0) return false
        if (file.progress >= 1.0) { spans = listOf(Span(0f, 1f, 1f)); return false }
        val offset = ordered.takeWhile { it.index != index }.sumOf { it.length }
        playing = Playing(id, index, offset, file.length)
        return true
    }

    /** The torrent's piece map, in buckets of whole pieces. */
    fun pieces(pieceLength: Long, buckets: List<PieceBucket>) {
        val p = playing ?: return
        if (pieceLength <= 0) return
        val end = p.offset + p.length
        spans = buckets.mapNotNull { b ->
            val a = maxOf(b.start * pieceLength, p.offset)
            val z = minOf((b.end + 1L) * pieceLength, end)
            if (z <= a || b.total <= 0) null
            else Span(
                ((a - p.offset).toDouble() / p.length).toFloat(),
                ((z - p.offset).toDouble() / p.length).toFloat(),
                b.verified.toFloat() / b.total,
            )
        }
    }

    /** True when the spot at [fraction] of the file is downloaded; null when unknown. */
    fun readyAt(fraction: Float): Boolean? {
        val list = spans.takeIf { it.isNotEmpty() } ?: return null
        return list.firstOrNull { fraction >= it.from && fraction <= it.to }?.let { it.done >= READY_SHARE }
    }

    fun readyAt(seconds: Double, duration: Double): Boolean? =
        if (duration > 0) readyAt((seconds / duration).toFloat()) else null

    /**
     * Bytes to prefetch so a seek to [seconds] starts on data already arriving, or
     * null when that spot is ready or the file is unknown. Time maps to bytes by the
     * file's average bitrate.
     */
    fun aimAt(seconds: Double, duration: Double): Aim? {
        val p = playing ?: return null
        if (duration <= 0) return null
        if (readyAt(seconds, duration) == true) return null
        val estimate = (p.length * (seconds / duration)).toLong()
        return Aim(p.id, p.index, estimate - PREFETCH_BEFORE, PREFETCH_LENGTH)
    }

    companion object {
        const val READY_SHARE = 0.99f
        /** Wait this long after the finger moves, so only the spot it rests on is fetched. */
        const val PREFETCH_DELAY_MS = 150L
        const val PREFETCH_BEFORE = 2L shl 20
        const val PREFETCH_LENGTH = 10L shl 20

        /** Engine id and file index from the path of http://127.0.0.1:port/torrent/<id>/file/<n>. */
        fun streamOf(scheme: String?, path: List<String>): Pair<String, Int>? {
            if (scheme != "http") return null
            val t = path.indexOf("torrent")
            if (t < 0 || path.size < t + 4 || path[t + 2] != "file") return null
            return path[t + 1] to (path[t + 3].toIntOrNull() ?: return null)
        }
    }
}

package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.media.MediaDataSource
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.os.SystemClock
import android.util.LruCache

/**
 * Seek-bar pictures, like YouTube: a small frame at the spot the finger is on.
 *
 * Torrent files are read through [EngineHost.readSaved], which only copies bytes
 * already saved (downloaded pieces or the Direct Play cache) and never downloads.
 * A spot that is not saved yet simply has no picture. Call from one thread only.
 */
class ScrubThumbs(private val context: Context, private val host: EngineHost) {
    sealed interface Source {
        data class Torrent(val id: String, val index: Int, val length: Long) : Source
        data class Local(val uri: Uri) : Source
    }

    private var source: Source? = null
    private var retriever: MediaMetadataRetriever? = null
    private var failedAt = 0L
    private val cache = LruCache<Long, Bitmap>(48)

    /** Nearest keyframe picture at [timeUs], or null when that spot is not saved. */
    fun frame(src: Source, timeUs: Long): Bitmap? {
        if (src != source) { release(); source = src }
        val bucket = timeUs / BUCKET_US
        cache.get(bucket)?.let { return it }
        val r = retriever ?: open(src) ?: return null
        val bmp = runCatching {
            if (Build.VERSION.SDK_INT >= 27)
                r.getScaledFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, WIDTH, WIDTH * 9 / 16)
            else r.getFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
        }.getOrNull() ?: return null
        cache.put(bucket, bmp)
        return bmp
    }

    private fun open(src: Source): MediaMetadataRetriever? {
        // The header or index may not be saved yet: try again a bit later, not on every move.
        if (SystemClock.elapsedRealtime() - failedAt < RETRY_MS) return null
        val r = MediaMetadataRetriever()
        val ok = runCatching {
            when (src) {
                is Source.Local -> r.setDataSource(context, src.uri)
                is Source.Torrent -> r.setDataSource(SavedBytes(host, src))
            }
        }.isSuccess
        if (!ok) { runCatching { r.release() }; failedAt = SystemClock.elapsedRealtime(); return null }
        retriever = r
        return r
    }

    fun release() {
        runCatching { retriever?.release() }
        retriever = null
        failedAt = 0L
        cache.evictAll()
    }

    private class SavedBytes(private val host: EngineHost, private val src: Source.Torrent) : MediaDataSource() {
        private var temp = ByteArray(0)
        override fun getSize(): Long = src.length
        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (size <= 0) return 0
            if (position >= src.length) return -1
            val want = minOf(size, 1 shl 20)
            if (temp.size < want) temp = ByteArray(want)
            val n = runCatching { host.readSaved(src.id, src.index, position, want, temp) }.getOrDefault(-1)
            if (n <= 0) return -1
            System.arraycopy(temp, 0, buffer, offset, n)
            return n
        }
        override fun close() {}
    }

    private companion object {
        const val WIDTH = 320
        const val BUCKET_US = 2_000_000L
        const val RETRY_MS = 5_000L
    }
}

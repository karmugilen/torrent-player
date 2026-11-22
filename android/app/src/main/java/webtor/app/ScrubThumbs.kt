package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.media.MediaDataSource
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.util.Log
import android.util.LruCache

/**
 * Seek-bar pictures: a small frame at the spot the finger is on.
 *
 * Torrent files are read through [EngineHost.readSaved], which only copies bytes
 * already saved (downloaded pieces or the Direct Play cache) and never downloads.
 * When a spot is not saved, [frame] says which byte was missing, so the player
 * can fetch exactly that piece and ask again. Call from one thread only.
 */
class ScrubThumbs(private val context: Context, private val host: EngineHost) {
    sealed interface Source {
        data class Torrent(val id: String, val index: Int, val length: Long) : Source
        data class Local(val uri: Uri) : Source
    }

    sealed interface Result {
        class Picture(val bitmap: Bitmap) : Result
        /** Not saved yet. [offset] is the first file byte that was missing, if known. */
        class Missing(val offset: Long?) : Result
    }

    private var source: Source? = null
    private var retriever: MediaMetadataRetriever? = null
    private var bytes: SavedBytes? = null
    private val cache = LruCache<Long, Bitmap>(64)

    fun frame(src: Source, timeUs: Long): Result {
        if (src != source) { release(); source = src }
        val bucket = timeUs / BUCKET_US
        cache.get(bucket)?.let { return Result.Picture(it) }
        bytes?.firstMissing = null
        val r = retriever ?: open(src) ?: return Result.Missing(bytes?.firstMissing)
        val bmp = runCatching {
            if (Build.VERSION.SDK_INT >= 27)
                r.getScaledFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, WIDTH, WIDTH * 9 / 16)
            else r.getFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
        }.getOrNull()
        if (bmp == null) {
            val missing = bytes?.firstMissing
            Log.i(TAG, "no frame at ${timeUs / 1000} ms, missing byte $missing")
            // A failed read can leave the parser stuck: start clean next time.
            dropRetriever()
            return Result.Missing(missing)
        }
        Log.i(TAG, "frame at ${timeUs / 1000} ms")
        cache.put(bucket, bmp)
        return Result.Picture(bmp)
    }

    private fun open(src: Source): MediaMetadataRetriever? {
        val r = MediaMetadataRetriever()
        val ok = runCatching {
            when (src) {
                is Source.Local -> r.setDataSource(context, src.uri)
                is Source.Torrent -> r.setDataSource(SavedBytes(host, src).also { bytes = it })
            }
        }.onFailure { Log.i(TAG, "open $src failed, missing byte ${bytes?.firstMissing}: ${it.message}") }.isSuccess
        if (!ok) { runCatching { r.release() }; return null }
        retriever = r
        return r
    }

    private fun dropRetriever() {
        runCatching { retriever?.release() }
        retriever = null
    }

    fun release() {
        dropRetriever()
        bytes = null
        cache.evictAll()
    }

    private class SavedBytes(private val host: EngineHost, private val src: Source.Torrent) : MediaDataSource() {
        @Volatile var firstMissing: Long? = null
        private var temp = ByteArray(0)
        override fun getSize(): Long = src.length
        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (size <= 0) return 0
            if (position >= src.length) return -1
            val want = minOf(size, 1 shl 20)
            if (temp.size < want) temp = ByteArray(want)
            val n = runCatching { host.readSaved(src.id, src.index, position, want, temp) }.getOrDefault(-1)
            if (n <= 0) {
                if (firstMissing == null) firstMissing = position
                return -1
            }
            System.arraycopy(temp, 0, buffer, offset, n)
            return n
        }
        override fun close() {}
    }

    private companion object {
        const val TAG = "webtor-scrub"
        const val WIDTH = 320
        const val BUCKET_US = 2_000_000L
    }
}

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
    private var failures = 0

    fun frame(src: Source, timeUs: Long): Result {
        if (src != source) { release(); source = src }
        val bucket = timeUs / BUCKET_US
        cache.get(bucket)?.let { return Result.Picture(it) }
        bytes?.firstMissing = null
        val r = retriever ?: open(src) ?: return Result.Missing(bytes?.firstMissing)
        // Only bytes the frame itself needs count; opening may peek at others.
        bytes?.firstMissing = null
        val bmp = runCatching {
            if (Build.VERSION.SDK_INT >= 27)
                r.getScaledFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, WIDTH, WIDTH * 9 / 16)
            else r.getFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
        }.getOrNull()
        if (bmp == null) {
            val missing = bytes?.firstMissing
            note("no frame at ${timeUs / 1000} ms, missing byte $missing")
            // Reopening is slow and reads other pieces: only after ~6 s of misses.
            if (++failures >= 40) { dropRetriever(); failures = 0 }
            return Result.Missing(missing)
        }
        failures = 0
        note("frame at ${timeUs / 1000} ms")
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
        }.onFailure { note("open $src failed, missing byte ${bytes?.firstMissing}: ${it.message}") }.isSuccess
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

    /** Steps go to logcat and to a small file (adb pull .../files/scrub.log). */
    fun note(msg: String) {
        Log.i(TAG, msg)
        runCatching {
            val f = java.io.File(context.getExternalFilesDir(null), "scrub.log")
            if (f.length() > 256 * 1024) f.delete()
            f.appendText("${System.currentTimeMillis() % 1_000_000} $msg\n")
        }
    }

    private companion object {
        const val TAG = "webtor-scrub"
        const val WIDTH = 320
        const val BUCKET_US = 2_000_000L
    }
}

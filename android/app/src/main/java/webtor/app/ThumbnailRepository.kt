package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.util.LinkedHashMap

/** Shared video-preview cache. Exactly three seeks from the first selected episode. */
class ThumbnailRepository(private val context: Context) {
    private val cache = File(context.cacheDir, "video-previews").apply { mkdirs() }
    private val memory = object : LinkedHashMap<String, CachedPreview>(24, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, CachedPreview>?) = size > 24
    }
    private val keyLocks = HashMap<String, Any>()

    private data class CachedPreview(val frames: List<Bitmap>, val complete: Boolean, val bucket: Int = 0)

    fun cached(entry: DownloadEntry): List<Bitmap>? {
        val video = firstPreviewVideo(entry) ?: return null
        val key = cacheKey(entry, video)
        return synchronized(memory) { memory[key]?.frames?.takeIf { it.isNotEmpty() } }
    }

    /**
     * Returns 1–3 swipeable frames from the first episode. Grabs as soon as a
     * URI exists, then replaces them each time downloaded progress crosses a
     * 5% step. Complete files keep a stable 20/50/80 set.
     */
    suspend fun previews(entry: DownloadEntry): List<Bitmap> = withContext(Dispatchers.IO) {
        val video = firstPreviewVideo(entry) ?: return@withContext emptyList()
        val uri = video.uri?.takeIf { it.isNotBlank() } ?: return@withContext emptyList()
        val complete = entry.complete || video.progress >= 1.0
        val bucket = previewUpdateBucket(video.progress, complete)
        val key = cacheKey(entry, video)
        val lock = synchronized(keyLocks) { keyLocks.getOrPut(key) { Any() } }
        synchronized(lock) { loadOrExtract(key, uri, video.progress, complete, bucket) }
    }

    private fun loadOrExtract(key: String, uri: String, progress: Double, complete: Boolean, bucket: Int): List<Bitmap> {
        val mem = synchronized(memory) { memory[key] }
        if (mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames
        // In-progress: skip only when this bucket already produced frames. Empty is not a hit.
        if (!complete && mem != null && mem.frames.isNotEmpty() && mem.bucket == bucket) return mem.frames

        val diskComplete = File(cache, "$key.done").exists()
        val disk = (0..2).mapNotNull { BitmapFactory.decodeFile(File(cache, "$key-$it.jpg").path) }
        if (diskComplete && disk.isNotEmpty()) {
            synchronized(memory) { memory[key] = CachedPreview(disk, true, 20) }
            return disk
        }
        val fallback = mem?.frames?.takeIf { it.isNotEmpty() } ?: disk.takeIf { it.isNotEmpty() }.orEmpty()
        if (complete && File(cache, "$key.failed").exists() && fallback.isEmpty()) return emptyList()

        val result = extractFrames(uri, progress, complete)
        if (result.isEmpty()) {
            if (complete && fallback.isEmpty()) File(cache, "$key.failed").writeText("failed")
            if (complete && fallback.isNotEmpty()) {
                synchronized(memory) { memory[key] = CachedPreview(fallback, true, 20) }
            }
            return fallback
        }
        for (i in 0 until 3) {
            val file = File(cache, "$key-$i.jpg")
            if (i < result.size) file.outputStream().use { result[i].compress(Bitmap.CompressFormat.JPEG, 82, it) }
            else file.delete()
        }
        if (complete) {
            File(cache, "$key.done").writeText("1")
            File(cache, "$key.failed").delete()
        }
        trimCache()
        synchronized(memory) { memory[key] = CachedPreview(result, complete, bucket) }
        return result
    }

    private fun extractFrames(uri: String, progress: Double, complete: Boolean): List<Bitmap> {
        var pfd: ParcelFileDescriptor? = null
        val retriever = MediaMetadataRetriever()
        return try {
            val parsed = Uri.parse(uri)
            try {
                retriever.setDataSource(context, parsed)
            } catch (_: Exception) {
                pfd = context.contentResolver.openFileDescriptor(parsed, "r") ?: return emptyList()
                retriever.setDataSource(pfd.fileDescriptor)
            }
            val duration = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0L
            val frames = ArrayList<Bitmap>(3)
            val tried = HashSet<Long>()
            fun take(timeUs: Long): Boolean {
                if (frames.size >= 3) return false
                val t = timeUs.coerceAtLeast(0L)
                if (!tried.add(t)) return false
                val bmp = grab(retriever, t, complete) ?: return false
                frames.add(bmp)
                return true
            }
            if (duration > 0L) {
                for (ratio in previewRatios(progress, complete)) {
                    if (frames.size >= 3) break
                    for (slot in previewSlotRatios(ratio, progress, complete)) {
                        if (take((duration * slot * 1000.0).toLong())) break
                    }
                }
                if (frames.isEmpty()) take(0L)
            } else {
                for (t in listOf(500_000L, 1_000_000L)) take(t)
                if (frames.isEmpty()) take(0L)
            }
            frames
        } catch (_: Exception) {
            emptyList()
        } finally {
            retriever.release()
            pfd?.close()
        }
    }

    private fun grab(retriever: MediaMetadataRetriever, timeUs: Long, complete: Boolean): Bitmap? {
        val raw = runCatching {
            if (complete) {
                frameAt(retriever, timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
            } else {
                frameAt(retriever, timeUs, MediaMetadataRetriever.OPTION_CLOSEST)
                    ?: frameAt(retriever, timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
                    ?: if (timeUs == 0L) frameAtIndex0(retriever) else null
            }
        }.getOrNull() ?: return null
        val scaled = scale(raw)
        if (scaled !== raw) raw.recycleQuietly()
        if (tooDark(scaled)) {
            scaled.recycleQuietly()
            return null
        }
        return scaled
    }

    private fun tooDark(bitmap: Bitmap): Boolean = runCatching {
        val sample = Bitmap.createScaledBitmap(bitmap, 16, 16, true)
        try {
            val pixels = IntArray(256)
            sample.getPixels(pixels, 0, 16, 0, 0, 16, 16)
            tooDarkLuma(pixels)
        } finally {
            if (sample !== bitmap) sample.recycleQuietly()
        }
    }.getOrDefault(false)

    private fun frameAt(retriever: MediaMetadataRetriever, timeUs: Long, option: Int): Bitmap? =
        if (Build.VERSION.SDK_INT >= 27) {
            retriever.getScaledFrameAtTime(timeUs, option, 720, 405)
                ?: retriever.getFrameAtTime(timeUs, option)
        } else {
            retriever.getFrameAtTime(timeUs, option)
        }

    private fun frameAtIndex0(retriever: MediaMetadataRetriever): Bitmap? {
        if (Build.VERSION.SDK_INT < 28) return null
        return runCatching { retriever.getFrameAtIndex(0) }.getOrNull()
    }

    private fun scale(bitmap: Bitmap): Bitmap {
        val ratio = minOf(720f / bitmap.width, 405f / bitmap.height, 1f)
        return if (ratio == 1f) bitmap else Bitmap.createScaledBitmap(
            bitmap, (bitmap.width * ratio).toInt().coerceAtLeast(1), (bitmap.height * ratio).toInt().coerceAtLeast(1), true,
        )
    }

    private fun Bitmap.recycleQuietly() {
        if (this.isRecycled) return
        runCatching { recycle() }
    }

    private fun trimCache() {
        val limit = 50L * 1024 * 1024
        var total = cache.listFiles()?.sumOf { it.length() } ?: return
        cache.listFiles()?.sortedBy { it.lastModified() }?.forEach { file ->
            if (total <= limit) return
            total -= file.length()
            file.delete()
        }
    }

    companion object {
        private fun cacheKey(entry: DownloadEntry, video: SavedFile): String =
            "v7-${(entry.key + video.index + video.path + video.length).hashCode()}"
    }
}

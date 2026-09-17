package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File
import java.util.LinkedHashMap
import java.util.concurrent.ConcurrentHashMap

/**
 * Mode C previews: up to three live frames from the on-disk file while
 * downloading (no engine seek / no download-priority change), then three
 * stable 20/50/80 frames once complete. Black/title-card frames are rejected
 * and nearby times are tried instead.
 */
class ThumbnailRepository(private val context: Context) {
    private val cache = File(context.cacheDir, "video-previews").apply { mkdirs() }
    private val memory = object : LinkedHashMap<String, CachedPreview>(24, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, CachedPreview>?) = size > 24
    }
    private val keyLocks = HashMap<String, Any>()
    private val extractSlots = Semaphore(MAX_EXTRACTORS)
    private val extractionScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val inFlight = HashMap<String, Deferred<List<Bitmap>>>()
    private val invalidation = ConcurrentHashMap<String, Long>()

    private data class CachedPreview(val frames: List<Bitmap>, val complete: Boolean, val progressBucket: Int)

    fun cached(entry: DownloadEntry): List<Bitmap>? {
        val video = firstPreviewVideo(entry) ?: return null
        val key = cacheKey(entry, video)
        return synchronized(memory) { memory[key]?.frames?.takeIf { it.isNotEmpty() } }
    }

    /**
     * Complete → up to 3 cached frames at 20/50/80.
     * In-progress → up to 3 frames from bytes already on disk (MediaStore URI).
     * Never opens PlaybackProvider / never steals piece priority for previews.
     * Callers should re-invoke every few seconds while live.
     */
    suspend fun previews(entry: DownloadEntry): List<Bitmap> {
        val video = firstPreviewVideo(entry) ?: return emptyList()
        val uri = video.uri?.takeIf { it.isNotBlank() } ?: return emptyList()
        val complete = entry.complete || video.isVerifiedComplete
        val key = cacheKey(entry, video)

        synchronized(memory) {
            val mem = memory[key]
            if (mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames
            if (mem != null && !mem.complete && mem.frames.isNotEmpty() &&
                mem.progressBucket == liveProgressBucket(video.progress)
            ) return mem.frames
        }

        val operationKey = "$key:${if (complete) "complete" else "live"}"
        val deferred = synchronized(inFlight) {
            inFlight[operationKey] ?: extractionScope.async {
                extractSlots.withPermit {
                    withTimeoutOrNull(EXTRACT_TIMEOUT_MS) {
                        val lock = synchronized(keyLocks) { keyLocks.getOrPut(key) { Any() } }
                        val version = invalidation[entry.key] ?: 0L
                        synchronized(lock) {
                            loadOrExtract(key, uri, video.progress, complete, version, entry.key)
                        }
                    } ?: emptyList()
                }
            }.also { inFlight[operationKey] = it }
        }
        deferred.invokeOnCompletion {
            synchronized(inFlight) {
                if (inFlight[operationKey] === deferred) inFlight.remove(operationKey)
            }
        }
        return try {
            deferred.await()
        } finally {
            if (deferred.isCompleted) synchronized(inFlight) {
                if (inFlight[operationKey] === deferred) inFlight.remove(operationKey)
            }
        }
    }

    fun invalidate(entry: DownloadEntry) {
        invalidateEntryKey(entry.key)
    }

    fun invalidateEntryKey(entryKey: String) {
        if (entryKey.isBlank()) return
        invalidation.compute(entryKey) { _, previous -> (previous ?: 0L) + 1L }
        val prefix = "$ALGORITHM-${sanitize(entryKey)}-"
        val removed = ArrayList<String>()
        synchronized(memory) {
            memory.keys.filter { it.startsWith(prefix) }.forEach {
                memory.remove(it)
                removed += it
            }
        }
        cache.listFiles()?.forEach { file ->
            val name = file.name
            if (removed.any { name.startsWith(it) } || name.startsWith(prefix)) {
                runCatching { file.delete() }
            }
        }
    }

    private fun loadOrExtract(
        key: String,
        uri: String,
        progress: Double,
        complete: Boolean,
        version: Long,
        entryKey: String,
    ): List<Bitmap> {
        val mem = synchronized(memory) { memory[key] }
        if (mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames

        if (complete) {
            val diskComplete = File(cache, "$key.done").exists()
            val disk = (0..2).mapNotNull { BitmapFactory.decodeFile(File(cache, "$key-$it.jpg").path) }
            if (diskComplete && disk.isNotEmpty()) {
                if (isInvalidated(entryKey, version)) return emptyList()
                putMemory(key, CachedPreview(disk, true, COMPLETE_BUCKET))
                return disk
            }
            if (File(cache, "$key.failed").exists() && mem?.frames.isNullOrEmpty()) return emptyList()
        }

        val fallback = mem?.frames.orEmpty()
        val result = if (complete) extractCompleteFrames(uri) else extractLiveFrames(uri, progress)
        if (result.isEmpty()) {
            if (complete && fallback.isEmpty()) File(cache, "$key.failed").writeText("failed")
            return fallback
        }
        if (isInvalidated(entryKey, version)) {
            result.forEach { it.recycleQuietly() }
            return emptyList()
        }
        if (complete) {
            for (i in 0 until 3) {
                val file = File(cache, "$key-$i.jpg")
                if (i < result.size) file.outputStream().use { result[i].compress(Bitmap.CompressFormat.JPEG, 82, it) }
                else file.delete()
            }
            File(cache, "$key.done").writeText("1")
            File(cache, "$key.failed").delete()
            File(cache, "$key-live.jpg").delete()
            trimCache()
            if (isInvalidated(entryKey, version)) {
                (0..2).forEach { File(cache, "$key-$it.jpg").delete() }
                File(cache, "$key.done").delete()
                result.forEach { it.recycleQuietly() }
                return emptyList()
            }
        }
        // Live frames stay memory-only so incomplete downloads do not fill disk.
        putMemory(key, CachedPreview(result, complete, if (complete) COMPLETE_BUCKET else liveProgressBucket(progress)))
        return result
    }

    private fun isInvalidated(entryKey: String, version: Long): Boolean =
        (invalidation[entryKey] ?: 0L) != version

    private fun putMemory(key: String, value: CachedPreview) {
        synchronized(memory) {
            memory[key] = value
            while (memoryBytes() > MAX_MEMORY_BYTES && memory.isNotEmpty()) {
                val eldest = memory.entries.iterator().next()
                if (eldest.key == key && memory.size == 1) break
                memory.remove(eldest.key)
            }
        }
    }

    private fun memoryBytes(): Long = memory.values.sumOf { cached ->
        cached.frames.sumOf { bitmap -> bitmap.allocationByteCount.toLong() }
    }

    private fun liveProgressBucket(progress: Double): Int = (progress.coerceIn(0.0, 1.0) * 100.0).toInt()

    fun clear() {
        extractionScope.cancel()
        synchronized(inFlight) { inFlight.clear() }
        synchronized(memory) { memory.clear() }
        synchronized(keyLocks) { keyLocks.clear() }
        invalidation.clear()
    }

    private fun extractCompleteFrames(uri: String): List<Bitmap> {
        return withRetriever(uri) { retriever, duration ->
            val frames = ArrayList<Bitmap>(3)
            val tried = HashSet<Long>()
            fun take(timeUs: Long): Boolean {
                if (frames.size >= 3) return false
                val bmp = grabNonDark(retriever, timeUs, duration, tried) ?: return false
                frames.add(bmp)
                return true
            }
            if (duration > 0L) {
                for (ratio in COMPLETE_RATIOS) {
                    take((duration * ratio * 1000.0).toLong())
                }
                if (frames.isEmpty()) take(0L)
            } else {
                for (t in listOf(500_000L, 1_000_000L, 2_000_000L)) take(t)
                if (frames.isEmpty()) take(0L)
            }
            frames
        }
    }

    private fun extractLiveFrames(uri: String, progress: Double): List<Bitmap> {
        return withRetriever(uri) { retriever, duration ->
            val frames = ArrayList<Bitmap>(3)
            val tried = HashSet<Long>()
            fun take(timeUs: Long): Boolean {
                if (frames.size >= 3) return false
                val bmp = grabNonDark(retriever, timeUs, duration, tried) ?: return false
                frames.add(bmp)
                return true
            }
            if (duration > 0L) {
                for (ratio in livePreviewRatios(progress)) {
                    take((duration * ratio * 1000.0).toLong())
                }
                if (frames.isEmpty()) take(0L)
            } else {
                // Duration unknown while the container is still filling —
                // probe a few absolute offsets that often decode early.
                for (t in listOf(500_000L, 1_500_000L, 3_000_000L, 0L)) take(t)
            }
            frames
        }
    }

    /**
     * Grab a frame at [timeUs], skipping black/title cards by nudging earlier
     * and later until a usable frame appears (or candidates are exhausted).
     */
    private fun grabNonDark(
        retriever: MediaMetadataRetriever,
        timeUs: Long,
        durationMs: Long,
        tried: MutableSet<Long>,
    ): Bitmap? {
        val durationUs = if (durationMs > 0L) durationMs * 1000L else Long.MAX_VALUE / 4
        val base = timeUs.coerceIn(0L, durationUs)
        val nudgesUs = listOf(
            0L,
            -1_000_000L, 1_000_000L,
            -2_500_000L, 2_500_000L,
            -5_000_000L, 5_000_000L,
            -10_000_000L, 10_000_000L,
            -20_000_000L, 20_000_000L,
        )
        for (nudge in nudgesUs) {
            val t = (base + nudge).coerceIn(0L, durationUs)
            // Quantize to 200ms so near-duplicates share one attempt.
            val key = t / 200_000L
            if (!tried.add(key)) continue
            val bmp = grab(retriever, t, rejectDark = true) ?: continue
            return bmp
        }
        return null
    }

    private fun withRetriever(uri: String, block: (MediaMetadataRetriever, Long) -> List<Bitmap>): List<Bitmap> {
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
            block(retriever, duration)
        } catch (_: Exception) {
            emptyList()
        } finally {
            retriever.release()
            pfd?.close()
        }
    }

    private fun grab(retriever: MediaMetadataRetriever, timeUs: Long, rejectDark: Boolean): Bitmap? {
        val raw = runCatching {
            frameAt(retriever, timeUs.coerceAtLeast(0L), MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
                ?: frameAt(retriever, timeUs.coerceAtLeast(0L), MediaMetadataRetriever.OPTION_CLOSEST)
                ?: if (timeUs <= 0L) frameAtIndex0(retriever) else null
        }.getOrNull() ?: return null
        val scaled = scale(raw)
        if (scaled !== raw) raw.recycleQuietly()
        if (rejectDark && tooDark(scaled)) {
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
        private const val MAX_EXTRACTORS = 2
        private const val EXTRACT_TIMEOUT_MS = 10_000L
        private const val ALGORITHM = "v9"
        private const val COMPLETE_BUCKET = 101
        private const val MAX_MEMORY_BYTES = 32L * 1024L * 1024L
        private val COMPLETE_RATIOS = listOf(0.20, 0.50, 0.80)

        fun cacheKey(entry: DownloadEntry, video: SavedFile): String =
            "$ALGORITHM-${sanitize(entry.key)}-${video.index}-${video.length}" +
                video.uri.orEmpty().takeIf { it.isNotBlank() }?.let { "-${sanitize(it)}" }.orEmpty()

        fun sanitize(value: String): String =
            value.lowercase().replace(Regex("[^a-z0-9_-]"), "_")
    }
}

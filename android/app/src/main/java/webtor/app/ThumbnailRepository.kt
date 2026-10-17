package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File
import java.util.LinkedHashMap
import java.util.concurrent.ConcurrentHashMap

data class MediaFacts(
    val durationMs: Long,
    val width: Int,
    val height: Int,
    val successfulOffsetUs: Long,
    val schemaVersion: Int = 1,
)

/**
 * MEDIA-01, MEDIA-02, CACHE-05, CACHE-07: Unified Thumbnail Lifecycle & Cache Repository.
 *
 * - Independent decoder fallbacks; dark real frame accepted as last successful fallback.
 * - Single extraction at a time, automatic on Wi-Fi and mobile, no manual gate.
 * - Survives process restart: incomplete & Direct Play first frames are persisted to disk.
 * - Stable cache key independent of provider URI or engine availability.
 * - Reusable clear(): clears caches without cancelling the extraction scope.
 * - Native preview JNI support for incomplete pieces without full destination file creation.
 * - Media facts (duration, dimensions, preview timestamp) cached with versioned keys.
 */
class ThumbnailRepository(
    private val context: Context,
    private val engineHost: EngineHost? = null,
) {
    private val cache = File(context.cacheDir, "video-previews").apply { mkdirs() }
    private val memory = object : LinkedHashMap<String, CachedPreview>(24, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, CachedPreview>?) = size > 24
    }
    private val keyLocks = HashMap<String, Any>()
    private val extractSlots = Semaphore(1)
    private val extractionScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val inFlight = HashMap<String, Deferred<List<Bitmap>>>()
    private val invalidation = ConcurrentHashMap<String, Long>()
    private val attemptHistory = ConcurrentHashMap<String, Pair<Int, Long>>()

    private data class CachedPreview(val frames: List<Bitmap>, val complete: Boolean, val progressBucket: Int)

    fun cached(entry: DownloadEntry): List<Bitmap>? {
        val video = firstPreviewVideo(entry) ?: return null
        val key = cacheKey(entry, video)
        synchronized(memory) {
            val mem = memory[key]
            if (mem != null && mem.frames.isNotEmpty()) return mem.frames
        }
        // Check disk cache hit for offline / paused / restored items
        val disk = loadFramesFromDisk(key)
        if (disk.isNotEmpty()) {
            val complete = entry.complete || video.isVerifiedComplete
            putMemory(key, CachedPreview(disk, complete, if (complete) COMPLETE_BUCKET else liveProgressBucket(video.progress)))
            return disk
        }
        return null
    }

    suspend fun previews(entry: DownloadEntry): List<Bitmap> {
        val video = firstPreviewVideo(entry) ?: return emptyList()
        val complete = entry.complete || video.isVerifiedComplete
        val key = cacheKey(entry, video)

        // Memory cache check
        synchronized(memory) {
            val mem = memory[key]
            if (mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames
            if (mem != null && !mem.complete && mem.frames.isNotEmpty() &&
                mem.progressBucket == liveProgressBucket(video.progress)
            ) return mem.frames
        }

        // Disk cache check (survives process restart)
        val disk = loadFramesFromDisk(key)
        if (disk.isNotEmpty()) {
            val isDone = File(cache, "$key.done").exists()
            if (isDone || !complete || disk.size >= 3) {
                putMemory(key, CachedPreview(disk, isDone || complete, if (isDone || complete) COMPLETE_BUCKET else liveProgressBucket(video.progress)))
                return disk
            }
        }

        // Bounded automatic retry check
        val now = SystemClock.elapsedRealtime()
        val attempts = attemptHistory[key]
        if (attempts != null && attempts.first >= MAX_RETRY_ATTEMPTS && (now - attempts.second) < RETRY_BACKOFF_MS) {
            return disk
        }

        val operationKey = "$key:${if (complete) "complete" else "live"}"
        val deferred = synchronized(inFlight) {
            inFlight[operationKey] ?: extractionScope.async {
                extractSlots.withPermit {
                    withTimeoutOrNull(EXTRACT_TIMEOUT_MS) {
                        val lock = synchronized(keyLocks) { keyLocks.getOrPut(key) { Any() } }
                        val version = invalidation[entry.key] ?: 0L
                        synchronized(lock) {
                            loadOrExtract(key, entry, video, complete, version)
                        }
                    } ?: disk
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
        attemptHistory.keys.filter { it.startsWith(prefix) }.forEach { attemptHistory.remove(it) }
        cache.listFiles()?.forEach { file ->
            val name = file.name
            if (removed.any { name.startsWith(it) } || name.startsWith(prefix)) {
                runCatching { file.delete() }
            }
        }
    }

    private fun loadOrExtract(
        key: String,
        entry: DownloadEntry,
        video: SavedFile,
        complete: Boolean,
        version: Long,
    ): List<Bitmap> {
        val mem = synchronized(memory) { memory[key] }
        if (mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames

        val disk = loadFramesFromDisk(key)
        val diskComplete = File(cache, "$key.done").exists()
        if (diskComplete && disk.isNotEmpty()) {
            if (isInvalidated(entry.key, version)) return emptyList()
            putMemory(key, CachedPreview(disk, true, COMPLETE_BUCKET))
            return disk
        }

        val fallback = if (disk.isNotEmpty()) disk else mem?.frames.orEmpty()
        val facts = readMediaFacts(key)

        var result: List<Bitmap> = emptyList()

        // Attempt 1: Local file / MediaStore URI extraction
        val uri = video.uri?.takeIf { it.isNotBlank() }
        if (uri != null) {
            result = if (complete) extractCompleteFrames(uri, facts, key) else extractLiveFrames(uri, video.progress, facts, key)
        }

        // Attempt 2: Native Preview JNI for missing/incomplete data
        if (result.isEmpty() && entry.engineId != null && engineHost != null) {
            result = extractNativePreview(entry.engineId, video.index, key)
        }

        if (result.isEmpty()) {
            // Record failure for bounded retry (never permanently poison cache)
            val currentAttempts = attemptHistory[key]?.first ?: 0
            attemptHistory[key] = (currentAttempts + 1) to SystemClock.elapsedRealtime()
            return fallback
        }

        // Successful extraction clears failure history
        attemptHistory.remove(key)

        if (isInvalidated(entry.key, version)) {
            result.forEach { it.recycleQuietly() }
            return emptyList()
        }

        // Atomically persist frames to disk
        // Even for live/incomplete, persist the first real frame so restart/pause retains it!
        persistFrames(key, result, complete)
        trimCache()

        if (isInvalidated(entry.key, version)) {
            deleteDiskFrames(key)
            result.forEach { it.recycleQuietly() }
            return emptyList()
        }

        putMemory(key, CachedPreview(result, complete, if (complete) COMPLETE_BUCKET else liveProgressBucket(video.progress)))
        return result
    }

    private fun extractNativePreview(engineId: String, fileIndex: Int, key: String): List<Bitmap> {
        val host = engineHost ?: return emptyList()
        val handle = runCatching { host.openPreview(engineId, fileIndex) }.getOrDefault(0L)
        if (handle == 0L) return emptyList()

        val tempFile = File(cache, "preview_tmp_${key}_${System.currentTimeMillis()}.bin")
        return try {
            val buffer = ByteArray(64 * 1024)
            var totalBytesRead = 0L
            val maxBudget = 8L * 1024L * 1024L // 8 MiB budget
            val startTime = SystemClock.elapsedRealtime()

            tempFile.outputStream().use { out ->
                while (totalBytesRead < maxBudget && (SystemClock.elapsedRealtime() - startTime) < 10_000L) {
                    val readSize = minOf(buffer.size.toLong(), maxBudget - totalBytesRead).toInt()
                    val count = host.readPreview(handle, totalBytesRead, readSize, buffer)
                    if (count <= 0) break
                    out.write(buffer, 0, count)
                    totalBytesRead += count
                }
            }

            if (totalBytesRead > 0 && tempFile.exists() && tempFile.length() > 0) {
                withRetriever(tempFile.absolutePath) { retriever, duration ->
                    val frame = grabNonDark(retriever, 0L, duration, HashSet())
                    if (frame != null) listOf(frame) else emptyList()
                }
            } else {
                emptyList()
            }
        } catch (_: Throwable) {
            emptyList()
        } finally {
            runCatching { host.closePreview(handle) }
            runCatching { tempFile.delete() }
        }
    }

    private fun loadFramesFromDisk(key: String): List<Bitmap> {
        val frames = ArrayList<Bitmap>(3)
        for (i in 0 until 3) {
            val file = File(cache, "$key-$i.jpg")
            if (file.exists() && file.length() > 0) {
                val bmp = runCatching { BitmapFactory.decodeFile(file.path) }.getOrNull()
                if (bmp != null) frames.add(bmp)
            }
        }
        return frames
    }

    private fun persistFrames(key: String, frames: List<Bitmap>, complete: Boolean) {
        val limit = if (complete) frames.size.coerceAtMost(3) else 1 // Persist first real frame for live/incomplete
        for (i in 0 until limit) {
            val target = File(cache, "$key-$i.jpg")
            val temp = File(cache, "$key-$i.jpg.tmp")
            try {
                temp.outputStream().use { frames[i].compress(Bitmap.CompressFormat.JPEG, 82, it) }
                if (temp.renameTo(target)) {
                    // Success
                } else {
                    target.delete()
                    temp.renameTo(target)
                }
            } catch (_: Throwable) {
                temp.delete()
            }
        }
        if (complete) {
            runCatching { File(cache, "$key.done").writeText("1") }
            runCatching { File(cache, "$key.live").delete() }
        } else {
            runCatching { File(cache, "$key.live").writeText("1") }
        }
    }

    private fun deleteDiskFrames(key: String) {
        (0..2).forEach { File(cache, "$key-$it.jpg").delete() }
        File(cache, "$key.done").delete()
        File(cache, "$key.live").delete()
        File(cache, "$key.facts").delete()
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

    /**
     * CACHE-07: Reusable cache clearing.
     * Does NOT cancel extractionScope, allowing future extraction requests to succeed.
     */
    fun clear() {
        synchronized(inFlight) {
            inFlight.values.forEach { it.cancel() }
            inFlight.clear()
        }
        synchronized(memory) { memory.clear() }
        synchronized(keyLocks) { keyLocks.clear() }
        invalidation.clear()
        attemptHistory.clear()
    }

    /**
     * Delete disk cache files and return reclaimed bytes.
     */
    fun clearDiskCache(): Long {
        clear()
        var reclaimed = 0L
        cache.listFiles()?.forEach { file ->
            val len = file.length()
            if (file.delete()) reclaimed += len
        }
        return reclaimed
    }

    fun cacheSizeBytes(): Long = cache.listFiles()?.sumOf { it.length() } ?: 0L

    private fun extractCompleteFrames(uri: String, facts: MediaFacts?, key: String): List<Bitmap> {
        return withRetriever(uri) { retriever, duration ->
            val actualDuration = if (duration > 0L) duration else facts?.durationMs ?: 0L
            val frames = ArrayList<Bitmap>(3)
            val tried = HashSet<Long>()
            var firstOffsetUs = facts?.successfulOffsetUs ?: -1L

            fun take(timeUs: Long): Boolean {
                if (frames.size >= 3) return false
                val bmp = grabNonDark(retriever, timeUs, actualDuration, tried) ?: return false
                if (firstOffsetUs < 0) firstOffsetUs = timeUs
                frames.add(bmp)
                return true
            }

            if (actualDuration > 0L) {
                for (ratio in COMPLETE_RATIOS) {
                    take((actualDuration * ratio * 1000.0).toLong())
                }
                if (frames.isEmpty()) take(0L)
            } else {
                for (t in listOf(500_000L, 1_000_000L, 2_000_000L)) take(t)
                if (frames.isEmpty()) take(0L)
            }

            if (frames.isNotEmpty() && actualDuration > 0L) {
                saveMediaFacts(key, retriever, actualDuration, firstOffsetUs.coerceAtLeast(0L))
            }
            frames
        }
    }

    private fun extractLiveFrames(uri: String, progress: Double, facts: MediaFacts?, key: String): List<Bitmap> {
        return withRetriever(uri) { retriever, duration ->
            val actualDuration = if (duration > 0L) duration else facts?.durationMs ?: 0L
            val frames = ArrayList<Bitmap>(3)
            val tried = HashSet<Long>()
            var firstOffsetUs = facts?.successfulOffsetUs ?: -1L

            fun take(timeUs: Long): Boolean {
                if (frames.size >= 3) return false
                val bmp = grabNonDark(retriever, timeUs, actualDuration, tried) ?: return false
                if (firstOffsetUs < 0) firstOffsetUs = timeUs
                frames.add(bmp)
                return true
            }

            if (actualDuration > 0L) {
                for (ratio in livePreviewRatios(progress)) {
                    take((actualDuration * ratio * 1000.0).toLong())
                }
                if (frames.isEmpty()) take(0L)
            } else {
                for (t in listOf(500_000L, 1_500_000L, 3_000_000L, 0L)) take(t)
            }

            if (frames.isNotEmpty()) {
                saveMediaFacts(key, retriever, actualDuration, firstOffsetUs.coerceAtLeast(0L))
            }
            frames
        }
    }

    /**
     * Independent decoder fallbacks and accepting a dark real frame as the last successful fallback.
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

        var darkFallbackFrame: Bitmap? = null

        for (nudge in nudgesUs) {
            val t = (base + nudge).coerceIn(0L, durationUs)
            val key = t / 200_000L
            if (!tried.add(key)) continue

            val raw = grabRawFrame(retriever, t) ?: continue
            val scaled = scale(raw)
            if (scaled !== raw) raw.recycleQuietly()

            if (!tooDark(scaled)) {
                darkFallbackFrame?.recycleQuietly()
                return scaled
            } else {
                if (darkFallbackFrame == null) {
                    darkFallbackFrame = scaled
                } else {
                    scaled.recycleQuietly()
                }
            }
        }

        // Accept real dark frame as the last fallback if non-dark frame was not found
        return darkFallbackFrame
    }

    private fun grabRawFrame(retriever: MediaMetadataRetriever, timeUs: Long): Bitmap? {
        // Fallback 1: CLOSEST_SYNC
        val frameSync = runCatching {
            frameAt(retriever, timeUs.coerceAtLeast(0L), MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
        }.getOrNull()
        if (frameSync != null) return frameSync

        // Fallback 2: CLOSEST
        val frameClosest = runCatching {
            frameAt(retriever, timeUs.coerceAtLeast(0L), MediaMetadataRetriever.OPTION_CLOSEST)
        }.getOrNull()
        if (frameClosest != null) return frameClosest

        // Fallback 3: Frame at index 0
        if (timeUs <= 0L) {
            val index0 = frameAtIndex0(retriever)
            if (index0 != null) return index0
        }

        return null
    }

    private fun withRetriever(uri: String, block: (MediaMetadataRetriever, Long) -> List<Bitmap>): List<Bitmap> {
        var pfd: ParcelFileDescriptor? = null
        val retriever = MediaMetadataRetriever()
        return try {
            if (uri.startsWith("content://") || uri.startsWith("file://")) {
                val parsed = Uri.parse(uri)
                try {
                    retriever.setDataSource(context, parsed)
                } catch (_: Exception) {
                    pfd = context.contentResolver.openFileDescriptor(parsed, "r") ?: return emptyList()
                    retriever.setDataSource(pfd.fileDescriptor)
                }
            } else {
                retriever.setDataSource(uri)
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

    private fun saveMediaFacts(key: String, retriever: MediaMetadataRetriever, durationMs: Long, offsetUs: Long) {
        val width = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)?.toIntOrNull() ?: 0
        val height = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toIntOrNull() ?: 0
        val facts = MediaFacts(durationMs, width, height, offsetUs)
        val file = File(cache, "$key.facts")
        val tmp = File(cache, "$key.facts.tmp")
        runCatching {
            tmp.writeText("${facts.durationMs},${facts.width},${facts.height},${facts.successfulOffsetUs},${facts.schemaVersion}")
            if (!tmp.renameTo(file)) {
                file.delete()
                tmp.renameTo(file)
            }
        }
    }

    private fun readMediaFacts(key: String): MediaFacts? {
        val file = File(cache, "$key.facts")
        if (!file.exists()) return null
        return runCatching {
            val parts = file.readText().trim().split(',')
            if (parts.size >= 4) {
                MediaFacts(
                    durationMs = parts[0].toLong(),
                    width = parts[1].toInt(),
                    height = parts[2].toInt(),
                    successfulOffsetUs = parts[3].toLong(),
                    schemaVersion = parts.getOrNull(4)?.toIntOrNull() ?: 1,
                )
            } else null
        }.getOrNull()
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
        private const val EXTRACT_TIMEOUT_MS = 10_000L
        private const val ALGORITHM = "v9"
        private const val COMPLETE_BUCKET = 101
        private const val MAX_MEMORY_BYTES = 32L * 1024L * 1024L
        private const val MAX_RETRY_ATTEMPTS = 3
        private const val RETRY_BACKOFF_MS = 15_000L
        private val COMPLETE_RATIOS = listOf(0.20, 0.50, 0.80)

        fun cacheKey(entry: DownloadEntry, video: SavedFile): String =
            "$ALGORITHM-${sanitize(entry.key)}-${video.index}-${video.length}"

        fun sanitize(value: String): String =
            value.lowercase().replace(Regex("[^a-z0-9_-]"), "_")
    }
}

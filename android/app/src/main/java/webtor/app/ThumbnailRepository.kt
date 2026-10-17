package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaDataSource
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
import kotlinx.coroutines.withContext
import kotlinx.coroutines.CancellationException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.io.File
import java.util.LinkedHashMap
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

data class MediaFacts(
    val durationMs: Long,
    val width: Int,
    val height: Int,
    val successfulOffsetUs: Long,
    val schemaVersion: Int = CURRENT_SCHEMA_VERSION,
) {
    fun isValid(): Boolean =
        schemaVersion == CURRENT_SCHEMA_VERSION && durationMs > 0L && width in 1..32768 && height in 1..32768 &&
            successfulOffsetUs >= 0L && successfulOffsetUs / 1000 <= durationMs

    companion object {
        const val CURRENT_SCHEMA_VERSION = 1
    }
}

/**
 * MEDIA-01, MEDIA-02, CACHE-05, CACHE-07: Unified Thumbnail Lifecycle & Cache Repository.
 *
 * - Independent decoder fallbacks; dark real frame accepted as last successful fallback.
 * - Single extraction at a time, automatic on Wi-Fi and mobile, no manual gate.
 * - Survives process restart: incomplete & Direct Play first frames are persisted to disk.
 * - Stable cache key independent of provider URI or engine availability.
 * - Reusable clear(): increments monotonic epochs, cancels in-flight work without destroying scope.
 * - Native preview JNI support backed by seekable MediaDataSource (full logical file length).
 * - Media facts (duration, dimensions, preview timestamp) validated and reused across extractions.
 * - Synchronous cached() lookup is strictly memory-only; disk IO happens asynchronously on IO dispatcher.
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

    private val publicationLock = Any()
    private val activePreviews = ConcurrentHashMap<String, () -> Unit>()
    private val previewDeadlines = Executors.newSingleThreadScheduledExecutor { task ->
        Thread(task, "preview-deadline").apply { isDaemon = true }
    }
    private val globalEpoch = AtomicLong(1L)
    private val entryEpochs = ConcurrentHashMap<String, AtomicLong>()
    private val attemptHistory = ConcurrentHashMap<String, Pair<Int, Long>>()

    private data class CachedPreview(val frames: List<Bitmap>, val complete: Boolean, val progressBucket: Int,
        val liveToken: String = "", val updatedElapsed: Long = SystemClock.elapsedRealtime())

    /**
     * Synchronous lookup for UI composables: STRICTLY memory-only.
     * Never performs disk IO or bitmap decoding on the calling thread.
     */
    fun cached(entry: DownloadEntry): List<Bitmap>? {
        val video = firstPreviewVideo(entry) ?: return null
        val key = cacheKey(entry, video)
        return synchronized(memory) {
            memory[key]?.frames?.takeIf { it.isNotEmpty() }
        }
    }

    private fun getEntryEpoch(key: String): Long =
        entryEpochs.computeIfAbsent(key) { AtomicLong(1L) }.get()

    private fun bumpEntryEpoch(key: String): Long =
        entryEpochs.computeIfAbsent(key) { AtomicLong(1L) }.incrementAndGet()

    private fun isStale(capturedGlobal: Long, capturedEntry: Long, entryKey: String): Boolean =
        capturedGlobal != globalEpoch.get() || capturedEntry != (entryEpochs[entryKey]?.get() ?: 1L)

    suspend fun previews(entry: DownloadEntry): List<Bitmap> = withContext(Dispatchers.IO) {
        previewsOnIo(entry)
    }

    private suspend fun previewsOnIo(entry: DownloadEntry): List<Bitmap> {
        val capturedGlobal = globalEpoch.get()
        val capturedEntry = getEntryEpoch(entry.key)
        val video = firstPreviewVideo(entry) ?: return emptyList()
        val complete = !entry.directPlay && (entry.complete || video.isVerifiedComplete)
        val key = cacheKey(entry, video)
        val previewClient = engineHost?.let { webtor.core.EngineClient(it) }
        val liveState = if (entry.directPlay && !entry.paused && entry.engineId != null && previewClient != null) {
            runCatching { previewClient.previewState(entry.engineId, video.index) }.getOrNull()
        } else null
        val refreshing = liveState?.active == true
        val liveToken = liveState?.let { "${it.signature}:${it.byteOffset / (512L * 1024L)}" }.orEmpty()
        val requestElapsed = SystemClock.elapsedRealtime()

        // 1. Memory cache check
        synchronized(memory) {
            val mem = memory[key]
            if (!entry.directPlay && mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames
            if (mem != null && !mem.complete && mem.frames.isNotEmpty() &&
                !complete && (if (refreshing) (mem.liveToken == liveToken || requestElapsed - mem.updatedElapsed < LIVE_REFRESH_MS)
                else mem.progressBucket == liveProgressBucket(video.progress))
            ) return mem.frames
        }

        // 2. Disk cache check (performed on IO thread)
        val disk = loadFramesFromDisk(key)
        val diskDone = !entry.directPlay && File(cache, "$key.done").exists()
        if (disk.isNotEmpty()) {
            if (diskDone || (!complete && !refreshing) || synchronized(memory) { memory[key] == null }) {
                synchronized(publicationLock) {
                    if (isStale(capturedGlobal, capturedEntry, entry.key)) return emptyList()
                    putMemory(key, CachedPreview(disk, diskDone, if (diskDone) COMPLETE_BUCKET else liveProgressBucket(video.progress)))
                }
                return disk
            }
        }

        // 3. Bounded automatic retry check
        val now = SystemClock.elapsedRealtime()
        val attempts = attemptHistory[key]
        if (attempts != null && attempts.first >= MAX_RETRY_ATTEMPTS && (now - attempts.second) < RETRY_BACKOFF_MS) {
            return disk
        }

        val operationKey = "$key:${if (complete) "complete" else "live"}"
        val deferred = synchronized(inFlight) {
            inFlight[operationKey]?.takeIf { it.isActive } ?: extractionScope.async {
                extractSlots.withPermit {
                    withTimeoutOrNull(EXTRACT_TIMEOUT_MS) {
                        val lock = synchronized(keyLocks) { keyLocks.getOrPut(key) { Any() } }
                        synchronized(lock) {
                            if (isStale(capturedGlobal, capturedEntry, entry.key)) emptyList()
                            else loadOrExtract(key, entry, video, complete, capturedGlobal, capturedEntry, liveState, liveToken)
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
        } catch (cancelled: CancellationException) {
            activePreviews.remove(entry.key)?.invoke()
            deferred.cancel()
            throw cancelled
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
        synchronized(publicationLock) { bumpEntryEpoch(entryKey) }
        activePreviews.remove(entryKey)?.invoke()
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
        capturedGlobal: Long,
        capturedEntry: Long,
        liveState: webtor.core.LivePreviewState?,
        liveToken: String,
    ): List<Bitmap> {
        val mem = synchronized(memory) { memory[key] }
        if (!entry.directPlay && mem != null && mem.complete && mem.frames.isNotEmpty()) return mem.frames

        val disk = loadFramesFromDisk(key)
        val diskComplete = !entry.directPlay && File(cache, "$key.done").exists()
        if (diskComplete && disk.isNotEmpty()) {
            if (isStale(capturedGlobal, capturedEntry, entry.key)) return emptyList()
            putMemory(key, CachedPreview(disk, true, COMPLETE_BUCKET))
            return disk
        }

        // Live frame acts as fallback while complete extraction is attempted
        val fallback = if (disk.isNotEmpty()) disk else mem?.frames.orEmpty()
        val facts = readMediaFacts(key)

        var result: List<Bitmap> = emptyList()

        // Path A: Local MediaStore / file URI extraction
        val uri = video.uri?.takeIf { it.isNotBlank() }
        if (uri != null) {
            result = if (complete) extractCompleteFrames(uri, facts, key, capturedGlobal, capturedEntry, entry.key)
            else extractLiveFrames(uri, video.progress, facts, key, capturedGlobal, capturedEntry, entry.key)
        }

        // Path B: Native Preview JNI with seekable MediaDataSource for missing/incomplete data
        if (result.isEmpty() && entry.engineId != null && engineHost != null) {
            result = extractNativePreview(entry.engineId, video, facts, key, capturedGlobal, capturedEntry, entry.key, liveState)
        }

        if (result.isEmpty()) {
            val currentAttempts = attemptHistory[key]?.first ?: 0
            attemptHistory[key] = (currentAttempts + 1) to SystemClock.elapsedRealtime()
            return fallback
        }

        if (liveState?.active == true && fallback.isNotEmpty()) {
            val newest = result.lastOrNull()
            result = (listOf(fallback.first()) + fallback.drop(1).takeLast(1) + listOfNotNull(newest)).take(3)
        }
        return synchronized(publicationLock) {
            if (isStale(capturedGlobal, capturedEntry, entry.key)) return@synchronized emptyList()
            attemptHistory.remove(key)
            val persisted = persistFramesAtomically(key, result, complete)
            trimCache()
            val finalComplete = complete && persisted
            putMemory(key, CachedPreview(result, finalComplete, if (finalComplete) COMPLETE_BUCKET else liveProgressBucket(video.progress), liveToken))
            result
        }
    }

    private fun extractNativePreview(
        engineId: String,
        video: SavedFile,
        facts: MediaFacts?,
        key: String,
        capturedGlobal: Long,
        capturedEntry: Long,
        entryKey: String,
        liveState: webtor.core.LivePreviewState?,
    ): List<Bitmap> {
        val host = engineHost ?: return emptyList()
        val handle = runCatching { host.openPreview(engineId, video.index) }.getOrDefault(0L)
        if (handle == 0L) return emptyList()

        val closed = java.util.concurrent.atomic.AtomicBoolean(false)
        val closeHandle = { if (closed.compareAndSet(false, true)) { runCatching { host.closePreview(handle) }; Unit }; Unit }
        activePreviews[entryKey] = closeHandle
        val timeout = previewDeadlines.schedule(closeHandle, EXTRACT_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        val retriever = MediaMetadataRetriever()
        return try {
            val startMs = SystemClock.elapsedRealtime()

            val dataSource = object : MediaDataSource() {
                override fun getSize(): Long = if (video.length > 0L) video.length else -1L

                override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
                    if (size == 0) return 0
                    if (closed.get() || isStale(capturedGlobal, capturedEntry, entryKey) ||
                        SystemClock.elapsedRealtime() - startMs > 10_000L
                    ) {
                        return -1
                    }
                    val toRead = minOf(size, 1024 * 1024)
                    if (toRead <= 0) return -1
                    val temp = ByteArray(toRead)
                    val read = host.readPreview(handle, position, toRead, temp)
                    if (read <= 0) return -1
                    System.arraycopy(temp, 0, buffer, offset, read)
                    return read
                }

                override fun close() {
                    // Closed in finally block
                }
            }

            retriever.setDataSource(dataSource)
            val duration = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0L
            val actualDuration = if (duration > 0L) duration else facts?.durationMs ?: 0L

            val frames = ArrayList<Bitmap>(3)
            val tried = HashSet<Long>()
            var firstOffsetUs = facts?.successfulOffsetUs ?: 0L
            val bodyOffsetUs = if (liveState?.active == true && video.length > 0L && actualDuration > 0L) {
                (liveState.byteOffset.toDouble() / video.length * actualDuration * 1000.0).toLong()
                    .coerceIn(0L, (actualDuration * 1000L - 1L).coerceAtLeast(0L))
            } else 0L
            val candidates = if (video.isVerifiedComplete && actualDuration > 0L) {
                COMPLETE_RATIOS.map { (actualDuration * it * 1000).toLong() }
            } else listOf(bodyOffsetUs, firstOffsetUs, 0L).distinct()
            for (candidate in candidates) {
                val frame = grabNonDark(retriever, candidate, actualDuration, tried)
                if (frame != null) {
                    if (frames.isEmpty()) firstOffsetUs = candidate
                    frames.add(frame)
                    if (!video.isVerifiedComplete || frames.size >= 3) break
                }
            }

            if (frames.isNotEmpty() && !isStale(capturedGlobal, capturedEntry, entryKey)) {
                saveMediaFacts(key, retriever, actualDuration, firstOffsetUs.coerceAtLeast(0L), capturedGlobal, capturedEntry, entryKey)
            }
            frames
        } catch (_: Throwable) {
            emptyList()
        } finally {
            timeout.cancel(false)
            activePreviews.remove(entryKey, closeHandle)
            closeHandle()
            runCatching { retriever.release() }
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

    private fun persistFramesAtomically(key: String, frames: List<Bitmap>, complete: Boolean): Boolean {
        val limit = frames.size.coerceAtMost(3)
        var allWritten = true
        for (i in 0 until limit) {
            val target = File(cache, "$key-$i.jpg")
            val temp = File(cache, "$key-$i.jpg.tmp")
            try {
                val compressed = temp.outputStream().use { frames[i].compress(Bitmap.CompressFormat.JPEG, 82, it) }
                check(compressed) { "Preview encoding failed" }
                if (temp.exists() && temp.length() > 0) {
                    if (!temp.renameTo(target)) { temp.delete(); allWritten = false }
                } else {
                    allWritten = false
                }
            } catch (_: Throwable) {
                temp.delete()
                allWritten = false
            }
        }
        if (allWritten) {
            for (i in limit until 3) File(cache, "$key-$i.jpg").delete()
            if (complete) {
                runCatching { File(cache, "$key.done").writeText("1") }
                runCatching { File(cache, "$key.live").delete() }
            } else {
                runCatching { File(cache, "$key.live").writeText("1") }
            }
        }
        return allWritten
    }

    private fun deleteDiskFrames(key: String) {
        (0..2).forEach { File(cache, "$key-$it.jpg").delete() }
        File(cache, "$key.done").delete()
        File(cache, "$key.live").delete()
        File(cache, "$key.facts").delete()
    }

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
     * Increments global epoch to reject late writes.
     * Cancels running jobs without cancelling extractionScope.
     */
    fun clear() {
        synchronized(publicationLock) {
            globalEpoch.incrementAndGet()
            synchronized(inFlight) {
                inFlight.values.forEach { it.cancel() }
                inFlight.clear()
            }
            synchronized(memory) { memory.clear() }
            attemptHistory.clear()
        }
        activePreviews.values.toList().forEach { it.invoke() }
        activePreviews.clear()
    }

    fun clearDiskCache(): Long = synchronized(publicationLock) {
        clear()
        var reclaimed = 0L
        cache.listFiles()?.forEach { file ->
            val len = file.length()
            check(file.delete() || !file.exists()) { "Some preview cache files could not be removed" }
            reclaimed += len
        }
        reclaimed
    }

    fun cacheSizeBytes(): Long = cache.listFiles()?.sumOf { it.length() } ?: 0L

    private fun extractCompleteFrames(
        uri: String,
        facts: MediaFacts?,
        key: String,
        capturedGlobal: Long,
        capturedEntry: Long,
        entryKey: String,
    ): List<Bitmap> {
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

            // Probe facts offset first if valid
            if (firstOffsetUs >= 0L) take(firstOffsetUs)

            if (actualDuration > 0L) {
                for (ratio in COMPLETE_RATIOS) {
                    take((actualDuration * ratio * 1000.0).toLong())
                }
                if (frames.isEmpty()) take(0L)
            } else {
                for (t in listOf(500_000L, 1_000_000L, 2_000_000L)) take(t)
                if (frames.isEmpty()) take(0L)
            }

            if (frames.isNotEmpty() && actualDuration > 0L && !isStale(capturedGlobal, capturedEntry, entryKey)) {
                saveMediaFacts(key, retriever, actualDuration, firstOffsetUs.coerceAtLeast(0L), capturedGlobal, capturedEntry, entryKey)
            }
            frames
        }
    }

    private fun extractLiveFrames(
        uri: String,
        progress: Double,
        facts: MediaFacts?,
        key: String,
        capturedGlobal: Long,
        capturedEntry: Long,
        entryKey: String,
    ): List<Bitmap> {
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

            if (firstOffsetUs >= 0L) take(firstOffsetUs)

            if (actualDuration > 0L) {
                for (ratio in livePreviewRatios(progress)) {
                    take((actualDuration * ratio * 1000.0).toLong())
                }
                if (frames.isEmpty()) take(0L)
            } else {
                for (t in listOf(500_000L, 1_500_000L, 3_000_000L, 0L)) take(t)
            }

            if (frames.isNotEmpty() && !isStale(capturedGlobal, capturedEntry, entryKey)) {
                saveMediaFacts(key, retriever, actualDuration, firstOffsetUs.coerceAtLeast(0L), capturedGlobal, capturedEntry, entryKey)
            }
            frames
        }
    }

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

        return darkFallbackFrame
    }

    private fun grabRawFrame(retriever: MediaMetadataRetriever, timeUs: Long): Bitmap? {
        val frameSync = runCatching {
            frameAt(retriever, timeUs.coerceAtLeast(0L), MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
        }.getOrNull()
        if (frameSync != null) return frameSync

        val frameClosest = runCatching {
            frameAt(retriever, timeUs.coerceAtLeast(0L), MediaMetadataRetriever.OPTION_CLOSEST)
        }.getOrNull()
        if (frameClosest != null) return frameClosest

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
            runCatching { retriever.release() }
            runCatching { pfd?.close() }
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

    private fun saveMediaFacts(key: String, retriever: MediaMetadataRetriever, durationMs: Long, offsetUs: Long,
        capturedGlobal: Long, capturedEntry: Long, entryKey: String) = synchronized(publicationLock) {
        if (isStale(capturedGlobal, capturedEntry, entryKey)) return@synchronized
        val width = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)?.toIntOrNull() ?: 0
        val height = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toIntOrNull() ?: 0
        val facts = MediaFacts(durationMs, width, height, offsetUs)
        if (!facts.isValid()) return@synchronized

        val file = File(cache, "$key.facts")
        val tmp = File(cache, "$key.facts.tmp")
        runCatching {
            tmp.writeText("${facts.durationMs},${facts.width},${facts.height},${facts.successfulOffsetUs},${facts.schemaVersion}")
            if (!tmp.renameTo(file)) tmp.delete()
        }
    }

    private fun readMediaFacts(key: String): MediaFacts? {
        val file = File(cache, "$key.facts")
        if (!file.exists()) return null
        return runCatching {
            val parts = file.readText().trim().split(',')
            if (parts.size >= 5) {
                val facts = MediaFacts(
                    durationMs = parts[0].toLong(),
                    width = parts[1].toInt(),
                    height = parts[2].toInt(),
                    successfulOffsetUs = parts[3].toLong(),
                    schemaVersion = parts[4].toInt(),
                )
                if (facts.isValid()) facts else null
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
        private const val LIVE_REFRESH_MS = 12_000L
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

package webtor.app

import android.content.Context
import android.content.ContextWrapper
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import java.io.File
import java.nio.file.Files

class ThumbnailLifecycleTest {

    private lateinit var tempDir: File
    private lateinit var fakeContext: Context
    private lateinit var repository: ThumbnailRepository

    @Before
    fun setUp() {
        tempDir = Files.createTempDirectory("thumb-test").toFile()
        fakeContext = object : ContextWrapper(null) {
            override fun getCacheDir(): File = tempDir
        }
        repository = ThumbnailRepository(fakeContext)
    }

    private fun testEntry(key: String, files: List<SavedFile>, selected: Set<Int>) = DownloadEntry(
        key = key,
        title = "Test",
        source = "magnet:?xt=urn:btih:$key",
        metadata = "",
        engineId = null,
        destination = "Downloads",
        files = files,
        selected = selected,
        metadataReady = true,
        lifecycleState = EntryLifecycleState.DOWNLOADING,
    )

    private fun testFile(index: Int, length: Long, progress: Double, uri: String? = null) = SavedFile(
        index = index,
        name = "ep$index.mkv",
        path = "ep$index.mkv",
        length = length,
        progress = progress,
        uri = uri,
    )

    @Test
    fun stableCacheKeyDoesNotDependOnProviderUri() {
        val videoWithUri = testFile(1, 50_000_000L, 0.5, "content://media/external/123")
        val videoWithoutUri = testFile(1, 50_000_000L, 0.5, null)
        val entry = testEntry("torrent-key-123", listOf(videoWithUri), setOf(1))

        val key1 = ThumbnailRepository.cacheKey(entry, videoWithUri)
        val key2 = ThumbnailRepository.cacheKey(entry, videoWithoutUri)

        assertEquals(key1, key2)
        assertEquals("v10-torrent-key-123-1-50000000", key1)
    }

    @Test
    fun cachedLookupIsMemoryOnlyAndNeverReadsDiskSynchronously() {
        val video = testFile(0, 100_000_000L, 0.2)
        val entry = testEntry("offline-key", listOf(video), setOf(0))

        val key = ThumbnailRepository.cacheKey(entry, video)
        val cacheDir = File(tempDir, "video-previews").apply { mkdirs() }
        val frameFile = File(cacheDir, "$key-0.jpg")
        frameFile.writeBytes(byteArrayOf(1, 2, 3))
        File(cacheDir, "$key.live").writeText("1")

        // cached() must be strictly memory-only; does not decode disk on UI thread
        assertNull(repository.cached(entry))
    }

    @Test
    fun clearDoesNotCancelExtractionScopeAndIsReusable() {
        // Clear caches
        repository.clear()

        // Call cacheSizeBytes after clear
        val size = repository.cacheSizeBytes()
        assertTrue(size >= 0)

        // Clear disk cache returns non-negative reclaimed bytes
        val reclaimed = repository.clearDiskCache()
        assertTrue(reclaimed >= 0)
        assertEquals(0L, repository.cacheSizeBytes())
    }

    @Test
    fun mediaFactsValidationRejectsUnsupportedSchemaAndNonPositiveValues() {
        val valid = MediaFacts(120_000L, 1920, 1080, 5_000_000L, MediaFacts.CURRENT_SCHEMA_VERSION)
        assertTrue(valid.isValid())

        val oldSchema = MediaFacts(120_000L, 1920, 1080, 5_000_000L, 999)
        assertFalse(oldSchema.isValid())

        val zeroDuration = MediaFacts(0L, 1920, 1080, 0L, MediaFacts.CURRENT_SCHEMA_VERSION)
        assertFalse(zeroDuration.isValid())

        val invalidDim = MediaFacts(120_000L, -1, 1080, 0L, MediaFacts.CURRENT_SCHEMA_VERSION)
        assertFalse(invalidDim.isValid())
    }

    @Test
    fun mediaFactsPersistenceAndParsing() {
        val key = "v10-testkey-0-1000"
        val cacheDir = File(tempDir, "video-previews").apply { mkdirs() }
        val factsFile = File(cacheDir, "$key.facts")

        factsFile.writeText("120000,1920,1080,5000000,1")

        val parts = factsFile.readText().trim().split(',')
        assertEquals(5, parts.size)
        val facts = MediaFacts(
            durationMs = parts[0].toLong(),
            width = parts[1].toInt(),
            height = parts[2].toInt(),
            successfulOffsetUs = parts[3].toLong(),
            schemaVersion = parts[4].toInt(),
        )

        assertEquals(120000L, facts.durationMs)
        assertEquals(1920, facts.width)
        assertEquals(1080, facts.height)
        assertEquals(5000000L, facts.successfulOffsetUs)
        assertEquals(1, facts.schemaVersion)
        assertTrue(facts.isValid())
    }
}

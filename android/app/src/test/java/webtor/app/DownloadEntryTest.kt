package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadEntryTest {
    @Test fun networkPolicyRequiresMatchingConnection() {
        assertTrue(transferAllowed(TransferNetworkPolicy.ANY, true, false, true))
        assertFalse(transferAllowed(TransferNetworkPolicy.WIFI_ONLY, true, false, false))
        assertTrue(transferAllowed(TransferNetworkPolicy.UNMETERED, true, true, false))
    }
    @Test
    fun diagnosticsReportDoesNotLeakSecrets() {
        val e = entry(listOf(file(0, 1, 0.0)), setOf(0)).copy(
            title = "secret-title", source = "magnet:?xt=urn:btih:ABC&tr=https://secret.example/x",
            metadata = "/private/path", error = "token=super-secret",
        )
        val report = buildDiagnosticsReport(listOf(e), "1.0", 1L)
        assertTrue(report.contains("entry[0].stage="))
        assertFalse(report.contains("secret-title"))
        assertFalse(report.contains("super-secret"))
        assertFalse(report.contains("private/path"))
        assertFalse(report.contains("magnet:"))
    }
    @Test
    fun librarySortIsStableAndSupportsFileSearch() {
        val first = entry(listOf(file(0, 1, 0.0, "episode.mkv")), setOf(0)).copy(key = "a", title = "Same", addedAt = 10)
        val second = entry(listOf(file(0, 1, 0.0, "notes.txt")), setOf(0)).copy(key = "b", title = "Same", addedAt = 10)
        assertEquals(listOf("a", "b"), listOf(first, second).stableLibrarySort(LibrarySort.NAME).map { it.key })
        assertEquals(listOf(first), listOf(first, second).filterLibrary("episode"))
    }

    @Test
    fun checkingDoesNotMarkCachedProgressCompleteOrDisablePause() {
        val status = webtor.core.EngineClient.parseTorrent(org.json.JSONObject(
            """{"id":"e1","checking":true,"checkedPieces":3,"checkTotal":10}""",
        ))
        val checking = entry(listOf(file(0, 100, 1.0)), setOf(0)).copy(
            engineId = "e1", status = status, lifecycleState = EntryLifecycleState.DOWNLOADING,
        )
        assertFalse(checking.complete)
        assertFalse(checking.controlsBusy())
        assertEquals("Checking saved data", checking.stateLabel())
        assertEquals(30, checking.checkPercent)
        assertEquals("Paused", checking.copy(paused = true).stateLabel())
        assertTrue(checking.copy(status = status.copy(checking = false)).complete)
    }

    private fun file(
        index: Int,
        length: Long,
        progress: Double,
        name: String = "f$index.bin",
        verifiedBytes: Long? = null,
    ) = SavedFile(
        index, name, name, length, uri = null, progress = progress,
        verifiedBytes = verifiedBytes ?: (length * progress.coerceIn(0.0, 1.0)).toLong(),
    )

    private fun entry(files: List<SavedFile>, selected: Set<Int>) =
        DownloadEntry("k", "title", "magnet:x", "", null, "Downloads", files, selected)

    @Test
    fun liveProgressDoesNotMarkCompleteUntilVerified() {
        val almost = entry(
            listOf(file(0, 100, progress = 1.0, verifiedBytes = 80)),
            setOf(0),
        )
        assertEquals(100L, almost.downloaded)
        assertEquals(1f, almost.progress)
        assertFalse(almost.complete)
        val done = entry(
            listOf(file(0, 100, progress = 1.0, verifiedBytes = 100)),
            setOf(0),
        )
        assertTrue(done.complete)
    }

    @Test
    fun totalSumsSelectedLengthsOnly() {
        val e = entry(
            listOf(file(0, 100, 1.0), file(1, 50, 0.0), file(2, 25, 0.5)),
            setOf(0, 2),
        )
        assertEquals(125L, e.total)
        assertEquals(112L, e.downloaded)
    }

    @Test
    fun downloadedUsesClampedProgress() {
        val e = entry(
            listOf(file(0, 100, 0.5), file(1, 40, 2.0), file(2, 20, -1.0)),
            setOf(0, 1, 2),
        )
        assertEquals(90L, e.downloaded)
        assertEquals(160L, e.total)
        assertEquals(90f / 160f, e.progress, 0.0001f)
    }

    @Test
    fun unselectedFilesDoNotAffectProgress() {
        val e = entry(
            listOf(file(0, 80, 1.0), file(1, 20, 0.0)),
            setOf(0),
        )
        assertEquals(80L, e.total)
        assertEquals(80L, e.downloaded)
        assertEquals(1f, e.progress)
        assertTrue(e.complete)
    }

    @Test
    fun completeRequiresEverySelectedFile() {
        val incomplete = entry(
            listOf(file(0, 100, 1.0), file(1, 50, 0.99)),
            setOf(0, 1),
        )
        assertFalse(incomplete.complete)
        val done = entry(
            listOf(file(0, 100, 1.0), file(1, 50, 1.0), file(2, 9, 0.0)),
            setOf(0, 1),
        )
        assertTrue(done.complete)
    }

    @Test
    fun zeroLengthSelectedFileCountsAsComplete() {
        val e = entry(listOf(file(0, 0, 0.0)), setOf(0))
        assertEquals(0L, e.total)
        assertEquals(0L, e.downloaded)
        assertEquals(1f, e.progress)
        assertTrue(e.complete)
    }

    @Test
    fun emptySelectionIsAnIntentionalStoppedState() {
        val e = entry(listOf(file(0, 100, 0.0)), emptySet())
        assertEquals(0L, e.total)
        assertEquals(0L, e.downloaded)
        assertEquals(0f, e.progress)
        assertFalse(e.complete)
    }

    @Test
    fun metadataPendingMagnetHasUnknownProgressAndIsNotComplete() {
        val e = DownloadEntry(
            key = "0123456789012345678901234567890123456789",
            title = "waiting",
            source = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
            metadata = "",
            engineId = "engine-1",
            destination = "Downloads/Webtor",
            files = emptyList(),
            selected = emptySet(),
            metadataReady = false,
        )
        assertEquals(0f, e.progress)
        assertFalse(e.complete)
        assertEquals("Connecting…", e.stateLabel())
    }

    @Test
    fun trackerIdentityRedactsAnnouncePathAndQuery() {
        assertEquals("https://tracker.example:443", redactedTrackerUrl("HTTPS://Tracker.Example:443/announce?token=secret"))
        assertEquals("udp://tracker.example:6969", redactedTrackerUrl("udp://tracker.example:6969/announce"))
        assertEquals("tracker", redactedTrackerUrl("not a tracker"))
    }

    @Test
    fun darkThemeIsOnByDefault() {
        assertTrue(UiState().darkTheme)
    }

    @Test
    fun looksLikeTorrentSourceAcceptsMagnetsAndHttp() {
        assertTrue(looksLikeTorrentSource("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"))
        assertTrue(looksLikeTorrentSource("MAGNET:?xt=urn:btih:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"))
        assertTrue(looksLikeTorrentSource("https://example.com/a.torrent"))
        assertFalse(looksLikeTorrentSource("magnet:?xt=urn:btih:short"))
        assertFalse(looksLikeTorrentSource("not a torrent"))
        assertFalse(looksLikeTorrentSource(""))
    }

    @Test
    fun addedAtDefaultsToNow() {
        val before = System.currentTimeMillis()
        val e = entry(listOf(file(0, 1, 0.0)), setOf(0))
        val after = System.currentTimeMillis()
        assertTrue(e.addedAt in before..after)
        val stamped = e.copy(addedAt = 1234L)
        assertEquals(1234L, stamped.addedAt)
    }

    @Test
    fun completedPlaybackUsesSelectedVideoWithoutEngine() {
        val e = entry(listOf(
            file(0, 100, 1.0, "movie.mp4").copy(uri = "content://media/1"),
            file(1, 200, 1.0, "notes.txt").copy(uri = "content://media/2"),
        ), setOf(0, 1))
        assertEquals(0, completedPlayFile(e)?.index)
        assertEquals(null, completedPlayFile(e.copy(files = e.files.map {
            it.copy(progress = 0.5, verifiedBytes = (it.length * 0.5).toLong())
        })))
    }

    @Test
    fun cleanupStaysInsideDownloadGroup() {
        val root = java.io.File("/tmp/Download/Webtor")
        assertEquals(listOf(java.io.File(root, "group/nested"), java.io.File(root, "group")),
            downloadDirectories(root, "Download/Webtor/group/nested/"))
        assertTrue(downloadDirectories(root, "Download/Webtor/").isEmpty())
        assertTrue(downloadDirectories(root, "Download/Webtor/../other").isEmpty())
        assertTrue(downloadDirectories(root, "Download/Other/group").isEmpty())
    }

    @Test
    fun draftAlwaysShowsPeerStatus() {
        val t = webtor.core.TorrentStatus("id", null, null, null, true, false, false,
            0.0, 0, 0, 0, 100, 0, 0, emptyList())
        val draft = PrepareDraft("id", "magnet:x", t, emptySet())
        assertEquals("Files ready · 0 peers", prefetchStatus(draft))
        assertEquals("Finding peers…", prefetchStatus(draft.copy(torrent = null)))
        assertEquals("Files ready · 2 peers", prefetchStatus(draft.copy(torrent = t.copy(numPeers = 2))))
        assertEquals("Files ready · 0 peers", prefetchStatus(draft.copy(torrent = t.copy(downloaded = 10))))
        assertEquals(null, prefetchStatus(null))
    }

    @Test
    fun everyVideoIsSelectedByDefault() {
        val files = listOf("a.mp4", "b.mkv", "readme.txt").mapIndexed { i, name ->
            webtor.core.TorrentFile(i, name, name, 100, 0.0, "application/octet-stream")
        }
        assertEquals(setOf(0, 1), defaultVideoSelection(files))
        assertEquals(55, UiState().maxPeers)
    }

    @Test
    fun busyLifecycleStatesDisableControls() {
        val base = entry(listOf(file(0, 100, 0.4)), setOf(0))
        val downloading = base.copy(engineId = "e1", lifecycleState = EntryLifecycleState.DOWNLOADING)
        assertFalse(downloading.controlsBusy())
        assertEquals("Downloading", downloading.stateLabel())
        assertEquals("Pausing…", downloading.copy(lifecycleState = EntryLifecycleState.PAUSING).stateLabel())
        assertTrue(downloading.copy(lifecycleState = EntryLifecycleState.PAUSING).controlsBusy())
        assertTrue(base.copy(isDeleting = true, lifecycleState = EntryLifecycleState.DELETING).controlsBusy())
        assertEquals("Paused", base.copy(paused = true, engineId = null, lifecycleState = EntryLifecycleState.STOPPED).stateLabel())
        assertEquals("Complete", base.copy(files = listOf(file(0, 100, 1.0)), lifecycleState = EntryLifecycleState.COMPLETED).stateLabel())
    }

    @Test
    fun canPlayWhenASelectedFileHasAUri() {
        val downloading = entry(listOf(file(0, 100, 0.4, "movie.mp4").copy(uri = "content://media/1")), setOf(0))
            .copy(engineId = "e1", lifecycleState = EntryLifecycleState.DOWNLOADING)
        assertTrue(downloading.canPlay())
        assertFalse(downloading.copy(files = listOf(file(0, 100, 0.4, "movie.mp4"))).canPlay())
        assertFalse(downloading.copy(isDeleting = true, lifecycleState = EntryLifecycleState.DELETING).canPlay())
        assertFalse(downloading.copy(lifecycleState = EntryLifecycleState.PAUSING).canPlay())
    }

    @Test
    fun selectedVideoCountIgnoresNonVideoAndUnselected() {
        val e = entry(listOf(
            file(0, 100, 1.0, "e1.mkv"),
            file(1, 100, 1.0, "e2.mkv"),
            file(2, 10, 1.0, "notes.txt"),
            file(3, 100, 1.0, "e3.mkv"),
        ), setOf(0, 1, 2))
        assertEquals(2, e.selectedVideoCount())
        assertEquals(1, entry(listOf(file(0, 100, 1.0, "movie.mp4")), setOf(0)).selectedVideoCount())
        assertEquals(0, entry(listOf(file(0, 100, 1.0, "track.flac")), setOf(0)).selectedVideoCount())
    }

    @Test
    fun previewRatiosModeCCompleteAndLiveThree() {
        assertEquals(listOf(0.20, 0.50, 0.80), previewRatios(0.4, true))
        assertEquals(listOf(0.20, 0.50, 0.80), previewRatios(1.0, false))
        assertEquals(listOf(0.10, 0.25, 0.40), previewRatios(0.5, false))
        assertEquals(3, livePreviewRatios(0.5).size)
        assertTrue(livePreviewRatios(0.5).all { it <= 0.5 + 1e-9 })
        assertEquals(1, livePreviewRatios(0.08).size)
        assertTrue(livePreviewRatios(0.01).isEmpty())
        assertEquals(0.25, livePreviewRatio(0.5), 1e-9)
        assertEquals(0.02, livePreviewRatio(0.0), 1e-9)
    }

    @Test
    fun previewSlotRatiosAreSingleTargetClampedToSpan() {
        assertEquals(listOf(0.20), previewSlotRatios(0.20, 0.4, true))
        assertEquals(listOf(0.50), previewSlotRatios(0.50, 0.5, false))
        assertEquals(listOf(0.018), previewSlotRatios(0.018, 0.0, false))
    }

    @Test
    fun tooDarkLumaRejectsBlackTitleCards() {
        fun argb(r: Int, g: Int, b: Int) = (0xFF shl 24) or (r shl 16) or (g shl 8) or b
        fun sample(r: Int, g: Int, b: Int) = IntArray(256) { argb(r, g, b) }
        assertTrue(tooDarkLuma(IntArray(0)))
        assertTrue(tooDarkLuma(sample(0, 0, 0)))
        assertTrue(tooDarkLuma(sample(17, 17, 17)))
        assertFalse(tooDarkLuma(sample(18, 18, 18)))
        assertFalse(tooDarkLuma(sample(255, 255, 255)))
        val mostlyBlack = IntArray(256) { i -> if (i == 0) argb(255, 255, 255) else argb(0, 0, 0) }
        assertTrue(tooDarkLuma(mostlyBlack))
    }

    @Test
    fun previewUpdateBucketStepsWithProgressThenLocksWhenComplete() {
        assertEquals(0, previewUpdateBucket(0.0, false))
        assertEquals(1, previewUpdateBucket(0.05, false))
        assertEquals(4, previewUpdateBucket(0.20, false))
        assertEquals(20, previewUpdateBucket(0.4, true))
        assertEquals(20, previewUpdateBucket(1.0, false))
    }

    @Test
    fun thumbnailCacheKeyUsesStableIdentityNotHashCode() {
        val video = file(2, 12345, 0.5, "ep.mkv")
        val e = entry(listOf(video), setOf(2)).copy(key = "AbCDef")
        assertEquals("v9-abcdef-2-12345", ThumbnailRepository.cacheKey(e, video))
        assertEquals("safe_name", ThumbnailRepository.sanitize("Safe/Name"))
    }

    @Test
    fun firstPreviewVideoPicksLowestIndexNotMostDownloaded() {
        val pack = entry(listOf(
            file(0, 100, 0.1, "e1.mkv").copy(uri = "content://media/1"),
            file(1, 100, 0.9, "e2.mkv").copy(uri = "content://media/2"),
            file(2, 100, 1.0, "e3.mkv").copy(uri = "content://media/3"),
        ), setOf(0, 1, 2))
        assertEquals(0, firstPreviewVideo(pack)?.index)
        val laterFirst = entry(listOf(
            file(0, 100, 1.0, "skipped.mkv"),
            file(2, 100, 0.1, "e1.mkv").copy(uri = "content://media/2"),
            file(5, 100, 0.95, "e2.mkv").copy(uri = "content://media/5"),
        ), setOf(2, 5))
        assertEquals(2, firstPreviewVideo(laterFirst)?.index)
        assertEquals(null, firstPreviewVideo(entry(listOf(file(0, 100, 1.0, "track.flac")), setOf(0))))
    }

    @Test
    fun restoreAllSkipsPausedCompleteAndDeleting() {
        val base = entry(listOf(file(0, 100, 0.4)), setOf(0))
        assertTrue(shouldSkipStartupRestore(base.copy(paused = true, lifecycleState = EntryLifecycleState.STOPPED)))
        assertTrue(shouldSkipStartupRestore(base.copy(files = listOf(file(0, 100, 1.0)), lifecycleState = EntryLifecycleState.COMPLETED)))
        assertTrue(shouldSkipStartupRestore(base.copy(isDeleting = true, lifecycleState = EntryLifecycleState.DELETING)))
        assertFalse(shouldSkipStartupRestore(base.copy(paused = false, engineId = null, lifecycleState = EntryLifecycleState.STOPPED)))
        assertFalse(shouldSkipStartupRestore(base.copy(paused = false, engineId = "e1", lifecycleState = EntryLifecycleState.DOWNLOADING)))
    }

    @Test
    fun restoreAppliesOnlyForMatchingCommandGeneration() {
        assertTrue(restoreStillApplies(2, 2, isDeleting = false, shutdownInProgress = false))
        assertFalse(restoreStillApplies(3, 2, isDeleting = false, shutdownInProgress = false))
        assertFalse(restoreStillApplies(2, 2, isDeleting = true, shutdownInProgress = false))
        assertFalse(restoreStillApplies(2, 2, isDeleting = false, shutdownInProgress = true))
        assertTrue(restoreStillApplies(5, 5, isDeleting = false, shutdownInProgress = false))
    }

    @Test
    fun explicitPlaybackChoosesRequestedEpisode() {
        val e = entry(listOf(
            file(0, 200, 1.0, "episode-1.mp4").copy(uri = "content://media/1"),
            file(1, 100, 1.0, "episode-2.mp4").copy(uri = "content://media/2"),
            file(2, 100, 1.0, "unselected.mp4").copy(uri = "content://media/3"),
        ), setOf(0, 1))
        assertEquals("content://media/2", completedPlayFile(e, 1)?.uri)
        assertEquals("content://media/1", completedPlayFile(e, 0)?.uri)
        // Deselecting a fully saved file stops transfer demand but must not
        // make its local media inaccessible.
        assertEquals("content://media/3", completedPlayFile(e, 2)?.uri)
        assertEquals(null, completedPlayFile(e, 99))
    }

    @Test
    fun stateLabelPreservesDownloadingWhenEngineIdNull() {
        val base = entry(listOf(file(0, 100, 0.4)), setOf(0))
        assertEquals("Downloading", base.copy(paused = false, engineId = null, lifecycleState = EntryLifecycleState.DOWNLOADING).stateLabel())
        assertEquals("Paused", base.copy(paused = true, engineId = null, lifecycleState = EntryLifecycleState.PAUSED).stateLabel())
        assertEquals("Paused", base.copy(paused = true, engineId = "e1", lifecycleState = EntryLifecycleState.PAUSED).stateLabel())
        assertEquals("Complete", base.copy(paused = false, files = listOf(file(0, 100, 1.0)), lifecycleState = EntryLifecycleState.COMPLETED).stateLabel())
    }
}

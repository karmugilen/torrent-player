package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
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
    fun libraryFiltersDistinguishActiveReadyAndNeedsAttention() {
        val active = entry(listOf(file(0, 100, 0.5)), setOf(0)).copy(key = "active", lifecycleState = EntryLifecycleState.DOWNLOADING)
        val ready = entry(listOf(file(0, 100, 1.0, verifiedBytes = 100)), setOf(0)).copy(key = "ready", lifecycleState = EntryLifecycleState.COMPLETED)
        val directPlay = entry(listOf(file(0, 100, 0.5, verifiedBytes = 50)), setOf(0)).copy(key = "dp", directPlay = true, lifecycleState = EntryLifecycleState.PAUSED)
        val error = entry(listOf(file(0, 100, 0.0)), setOf(0)).copy(key = "err", error = "Storage full", lifecycleState = EntryLifecycleState.ERROR)
        val all = listOf(active, ready, directPlay, error)

        assertEquals(listOf("active"), all.applyFilter(LibraryFilter.ACTIVE).map { it.key })
        assertEquals(listOf("ready", "dp"), all.applyFilter(LibraryFilter.READY).map { it.key })
        assertEquals(listOf("err"), all.applyFilter(LibraryFilter.NEEDS_ATTENTION).map { it.key })
        assertEquals(4, all.applyFilter(LibraryFilter.ALL).size)
    }

    @Test
    fun continueWatchingFindsRealBookmarkAndIgnoresEmptyPositions() {
        val noBookmark = entry(listOf(file(0, 100, 1.0, "ep0.mkv")), setOf(0))
        assertNull(listOf(noBookmark).findContinueWatchingTarget())

        // Non-video files with bookmark positions must be ignored
        val nonVideoBookmarked = file(0, 100, 1.0, "notes.txt").copy(uri = "content://downloads/episode", playbackPositionMs = 45_000L, playbackDurationMs = 120_000L)
        assertNull(listOf(entry(listOf(nonVideoBookmarked), setOf(0))).findContinueWatchingTarget())

        val bookmarkedFile = file(1, 100, 1.0, "ep1.mkv").copy(uri = "content://downloads/episode", playbackPositionMs = 45_000L, playbackDurationMs = 120_000L)
        val withBookmark = entry(listOf(file(0, 100, 1.0, "ep0.mkv"), bookmarkedFile), setOf(0, 1)).copy(key = "series", title = "Show S01")
        val target = listOf(withBookmark).findContinueWatchingTarget()
        assertTrue(target != null)
        assertEquals("series", target?.entry?.key)
        assertEquals(1, target?.file?.index)
        assertEquals(45_000L, target?.file?.playbackPositionMs)
    }

    @Test
    fun continueWatchingTargetsMostRecentlyBookmarkedEpisodeAndIgnoresFinishedEpisodes() {
        val ep1 = file(0, 100, 1.0, "Show.S01E01.mkv").copy(
            uri = "content://downloads/episode",
            playbackPositionMs = 45_000L,
            playbackDurationMs = 120_000L,
            bookmarkUpdatedAt = 1000L,
        )
        val ep2 = file(1, 100, 1.0, "Show.S01E02.mkv").copy(
            uri = "content://downloads/episode",
            playbackPositionMs = 30_000L,
            playbackDurationMs = 120_000L,
            bookmarkUpdatedAt = 2000L,
        )
        val series = entry(listOf(ep1, ep2), setOf(0, 1)).copy(key = "series", title = "Show S01")

        // Ep 2 has newer bookmarkUpdatedAt, so it must be selected
        val targetEp2 = listOf(series).findContinueWatchingTarget()
        assertEquals(1, targetEp2?.file?.index)

        // If Ep 2 is finished (within 3 seconds of duration), it should fall back to Ep 1
        val ep2Finished = ep2.copy(uri = "content://downloads/episode", playbackPositionMs = 118_000L, bookmarkUpdatedAt = 3000L)
        val seriesWithFinishedEp2 = series.copy(files = listOf(ep1, ep2Finished))
        val targetEp1 = listOf(seriesWithFinishedEp2).findContinueWatchingTarget()
        assertEquals(0, targetEp1?.file?.index)

        // Legacy records with bookmarkUpdatedAt == 0L fall back to entry.addedAt
        val legacy1 = entry(listOf(ep1.copy(bookmarkUpdatedAt = 0L)), setOf(0)).copy(key = "leg1", addedAt = 500L)
        val legacy2 = entry(listOf(ep2.copy(bookmarkUpdatedAt = 0L)), setOf(1)).copy(key = "leg2", addedAt = 1500L)
        val legacyTarget = listOf(legacy1, legacy2).findContinueWatchingTarget()
        assertEquals("leg2", legacyTarget?.entry?.key)
    }

    @Test
    fun naturalEpisodeOrderingSortsEpisodesProperly() {
        val ep1 = file(0, 100, 1.0, "Show.S01E01.mkv")
        val ep2 = file(1, 100, 1.0, "Show.S01E02.mkv")
        val ep10 = file(2, 100, 1.0, "Show.S01E10.mkv")
        val ep9 = file(3, 100, 1.0, "Show.S01E9.mkv")

        val sorted = listOf(ep10, ep2, ep9, ep1).sortedWith(compareBy<SavedFile> { naturalMediaKey(it.name) }.thenBy { it.index })
        assertEquals(listOf("Show.S01E01.mkv", "Show.S01E02.mkv", "Show.S01E9.mkv", "Show.S01E10.mkv"), sorted.map { it.name })
    }

    @Test
    fun defaultVideoSelectionIncludesPairedSubtitles() {
        val files = listOf(
            webtor.core.TorrentFile(0, "Movie.mp4", "Movie.mp4", 1_000_000L, 0.0, "video"),
            webtor.core.TorrentFile(1, "Movie.en.srt", "Movie.en.srt", 20_000L, 0.0, "text"),
            webtor.core.TorrentFile(2, "Unrelated.txt", "Unrelated.txt", 500L, 0.0, "text"),
        )
        val selected = defaultVideoSelection(files)
        assertTrue(0 in selected)
        assertTrue(1 in selected)
        assertFalse(2 in selected)
        assertTrue("Movie.en.srt".isSubtitleName())
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
        assertEquals("Waiting for peers", e.stateLabel())
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
        assertEquals("v11-abcdef-2-12345", ThumbnailRepository.cacheKey(e, video))
        assertEquals("safe_name", ThumbnailRepository.sanitize("Safe/Name"))
    }

    @Test
    fun episodePreviewFollowsFocusedFileInsteadOfRepeatingFirstEpisode() {
        val pack = entry(listOf(
            file(0, 100, 0.7, "Show.S01E01.mkv"),
            file(1, 100, 0.6, "Show.S01E02.mkv"),
        ), setOf(0, 1))
        val first = firstPreviewVideo(pack.copy(focusedFile = 0))!!
        val second = firstPreviewVideo(pack.copy(focusedFile = 1))!!
        assertEquals(0, first.index)
        assertEquals("Episode two must not reuse episode one's preview", 1, second.index)
        assertTrue(ThumbnailRepository.cacheKey(pack, first) != ThumbnailRepository.cacheKey(pack, second))
    }

    @Test
    fun episodePreviewPagesUseNaturalOrderAndDistinctFileIdentities() {
        val pack = entry(listOf(
            file(0, 100, 0.7, "Show.S01E10.mkv"),
            file(1, 100, 0.6, "Show.S01E02.mkv"),
            file(2, 100, 0.5, "Show.S01E01.mkv"),
            file(3, 100, 0.5, "Show.S01E03.mkv"),
            file(4, 100, 0.5, "Show.S01E01.srt"),
        ), setOf(0, 1, 2, 4))
        val pages = previewVideos(pack)
        assertEquals(listOf(2, 1, 0), pages.map { it.index })
        assertEquals(3, pages.map { ThumbnailRepository.cacheKey(pack, it) }.toSet().size)
        for (page in pages) assertEquals(page, previewVideo(pack, page.index))
        assertNull(previewVideo(pack, 3)) // Deselected: never reuse episode one's image.
        assertNull(previewVideo(pack, 4)) // Subtitle: never show a video image for it.
        assertNull(previewVideo(pack, 999))
        assertEquals(2, firstPreviewVideo(pack.copy(focusedFile = 999))?.index)
    }

    @Test
    fun middleFrameTargetsVaryByEpisodeButStayStableAcrossRefreshes() {
        val durationMs = 2_400_000L
        val first = previewTargetTimesUs("torrent/episode-1", durationMs)
        val second = previewTargetTimesUs("torrent/episode-2", durationMs)
        assertEquals(first, previewTargetTimesUs("torrent/episode-1", durationMs))
        assertTrue("Episodes must not all seek the same timestamps", first != second)
        assertEquals(3, first.distinct().size)
        assertTrue(first.all { it in 360_000_000L..2_040_000_000L })
    }

    @Test
    fun liveFrameTargetsStayNearPlaybackInsteadOfReturningTheIntro() {
        val targets = previewTargetTimesUs("torrent/episode-1", 2_400_000L, playbackOffsetUs = 900_000_000L)
        assertTrue(targets.all { it in 894_000_000L..900_000_000L })
        val later = previewTargetTimesUs("torrent/episode-1", 2_400_000L, playbackOffsetUs = 1_200_000_000L)
        assertTrue(later.all { it in 1_194_000_000L..1_200_000_000L })
        assertTrue(targets != later)
        assertTrue(previewTargetTimesUs("episode", 2_400_000L, availableRatio = 0.2)
            .all { it in 0L..480_000_000L })
    }

    @Test
    fun repeatedOrNearIdenticalFramesAreNotSeparatePreviews() {
        val scene = IntArray(144) { i -> if (i % 2 == 0) 0xff204080.toInt() else 0xff80a020.toInt() }
        assertFalse(previewFramesDiffer(scene, scene.copyOf()))
        val noise = scene.map { it + 0x010101 }.toIntArray()
        assertFalse(previewFramesDiffer(scene, noise))
        assertTrue(previewFramesDiffer(scene, IntArray(144) { 0xfff0d0a0.toInt() }))
        assertFalse(previewFramesDiffer(intArrayOf(), intArrayOf()))
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

    @Test
    fun pendingMagnetRestoreAndResumeDoNotRequireFileSelection() {
        val pendingMagnet = DownloadEntry(
            key = "0123456789012345678901234567890123456789",
            title = "waiting-magnet",
            source = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
            metadata = "",
            engineId = null,
            destination = "Downloads/Webtor",
            files = emptyList(),
            selected = emptySet(),
            metadataReady = false,
            paused = false,
            lifecycleState = EntryLifecycleState.DOWNLOADING,
        )
        val readyWithoutSelection = entry(listOf(file(0, 100, 0.0)), emptySet())

        // restoreValidationError must allow empty selected for metadata-pending magnets
        assertEquals(null, restoreValidationError(pendingMagnet))
        assertEquals("Select at least one file to download.", restoreValidationError(readyWithoutSelection))

        // requiresFileSelectionForResume must not block pending magnets
        assertFalse(requiresFileSelectionForResume(pendingMagnet))
        assertTrue(requiresFileSelectionForResume(readyWithoutSelection))

        // shouldSkipStartupRestore must not skip runnable pending magnets
        assertFalse(shouldSkipStartupRestore(pendingMagnet))
        assertTrue(shouldSkipStartupRestore(pendingMagnet.copy(paused = true)))
    }

    @Test
    fun pendingMagnetWaitingForPeersLabels() {
        val pendingMagnet = DownloadEntry(
            key = "0123456789012345678901234567890123456789",
            title = "waiting-magnet",
            source = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
            metadata = "",
            engineId = null,
            destination = "Downloads/Webtor",
            files = emptyList(),
            selected = emptySet(),
            metadataReady = false,
            paused = false,
            lifecycleState = EntryLifecycleState.DOWNLOADING,
        )

        // stateLabel: Waiting for peers when discovery is active, Paused when actually paused
        assertEquals("Waiting for peers", pendingMagnet.stateLabel())
        assertEquals("Waiting for peers", pendingMagnet.copy(engineId = "engine-1").stateLabel())
        assertEquals("Paused", pendingMagnet.copy(paused = true).stateLabel())
        assertEquals("Paused", pendingMagnet.copy(lifecycleState = EntryLifecycleState.PAUSED).stateLabel())
        assertEquals("Paused", pendingMagnet.copy(paused = true, engineId = "engine-1").stateLabel())

        // downloadSummaryLabel in DownloadSummary: engineId == null during pending discovery must show "Waiting for peers", NOT "Paused"
        assertEquals("Waiting for peers", downloadSummaryLabel(pendingMagnet))
        assertEquals("Waiting for peers", downloadSummaryLabel(pendingMagnet.copy(engineId = "engine-1")))
        assertEquals("Paused", downloadSummaryLabel(pendingMagnet.copy(paused = true)))
        assertEquals("Paused", downloadSummaryLabel(pendingMagnet.copy(paused = true, engineId = null)))

        val ready = entry(listOf(file(0, 100, 0.4)), setOf(0))
        assertEquals("Paused", downloadSummaryLabel(ready.copy(engineId = null)))
        assertEquals("Downloading", downloadSummaryLabel(ready.copy(engineId = "engine-1", lifecycleState = EntryLifecycleState.DOWNLOADING)))
        assertEquals("Paused", downloadSummaryLabel(ready.copy(engineId = "engine-1", lifecycleState = EntryLifecycleState.STOPPED)))
    }
}

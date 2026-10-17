package webtor.app

import org.json.JSONArray
import org.junit.Assert.*
import org.junit.Test

class DirectPlayTest {
    @Test fun resumeByteHintIsBoundedAndRequiresDuration() {
        val file = SavedFile(0, "movie.mp4", "movie.mp4", 1_000_000L,
            playbackPositionMs = 50_000L, playbackDurationMs = 100_000L)
        assertEquals(500_000L, file.resumeByteHint())
        assertNull(file.copy(playbackDurationMs = 0).resumeByteHint())
        assertEquals(0L, file.copy(playbackPositionMs = 0).resumeByteHint())
        assertNull(file.copy(length = 0).resumeByteHint())
        assertEquals(999_999L, file.copy(playbackPositionMs = Long.MAX_VALUE).resumeByteHint())
        assertTrue(file.copy(length = Long.MAX_VALUE).resumeByteHint()!! > 0)
    }
    private fun entry(direct: Boolean = true): DownloadEntry {
        val files = listOf("episode10.mkv", "episode2.mkv", "episode1.mkv", "episode2.en.srt", "episode1.srt", "notes.txt")
            .mapIndexed { index,name -> SavedFile(index,name,"show/$name",1000L, progress=1.0, verifiedBytes=1000L) }
        return DownloadEntry("0123456789012345678901234567890123456789","Show","magnet:x","metadata","native","Downloads",files,files.indices.toSet(),directPlay=direct)
    }
    @Test fun directPlayNeverPromisesOfflineCompletionAndNeedsNoDestination() {
        val entry = entry()
        assertFalse(entry.complete)
        assertTrue(entry.canPlay())
        assertNull(completedPlayFile(entry,0))
        assertFalse(shouldRestoreAtStartup(entry))
        assertNull(restoreValidationError(entry))
        assertNull(entry.selectedEta())
    }
    @Test fun savedDirectPlayStartsPausedAndForgetsVolatileDataButKeepsResume() {
        val entry = entry().copy(files=entry().files.map { it.copy(playbackPositionMs=123_000L,playbackDurationMs=500_000L) })
        val restored = DownloadStorage.parseEntry(JSONArray(DownloadStorage.encodeLibrary(listOf(entry))).getJSONObject(0))
        assertTrue(restored.directPlay)
        assertTrue(restored.paused)
        assertNull(restored.engineId)
        assertEquals(EntryLifecycleState.PAUSED,restored.lifecycleState)
        assertEquals(0L,restored.downloaded)
        assertEquals(0L,restored.files.first().verifiedBytes)
        assertEquals(123_000L,restored.files.first().playbackPositionMs)
        assertEquals(500_000L,restored.files.first().playbackDurationMs)
    }
    @Test fun olderDownloadSavesStillRestoreAsDownloads() {
        val entry = entry(false).copy(files=entry().files.map { it.copy(uri="content://saved/${it.index}") })
        val json = JSONArray(DownloadStorage.encodeLibrary(listOf(entry))).getJSONObject(0).apply { remove("directPlay") }
        val restored = DownloadStorage.parseEntry(json)
        assertFalse(restored.directPlay)
        assertFalse(restored.paused)
        assertTrue(restored.complete)
        assertEquals(6000L,restored.downloaded)
    }
    @Test fun playlistUsesEpisodeOrderAndMatchesLanguageSidecars() {
        val entry=entry()
        val files=playbackFiles(entry,entry.files[1])
        assertEquals(listOf("episode1.mkv","episode2.mkv","episode10.mkv"),files.videos.map {it.name})
        assertEquals(listOf("episode2.en.srt"),files.subtitles.map {it.name})
        assertEquals(2,pickPlayIndex(entry))
        assertTrue(playbackFiles(entry.copy(selected=setOf(1)),entry.files[1]).subtitles.isEmpty())
    }
    @Test fun playlistReturnMustIdentifyTheEpisodeToSaveItsPosition() {
        assertEquals("b",returnedPlaybackUri("b","a",setOf("a","b")))
        assertNull(returnedPlaybackUri(null,"a",setOf("a","b")))
        assertNull(returnedPlaybackUri("unknown","a",setOf("a","b")))
        assertEquals("a",returnedPlaybackUri(null,"a",setOf("a")))
    }
    @Test fun finishedPlaybackResetsResumeAndFormatsTimes() {
        val file=entry().files.first()
        assertEquals(0L,file.withPlaybackPosition(99_000,100_000).playbackPositionMs)
        assertEquals(50_000L,file.withPlaybackPosition(50_000,100_000).playbackPositionMs)
        assertEquals("2:03",playbackTime(123_000))
        assertEquals("1:00:01",playbackTime(3_601_000))
    }
    @Test fun streamingTelemetryDescribesCacheWithoutFullDownloadEta() {
        val entry=entry()
        assertTrue(entryTelemetry(entry).contains("cached"))
        assertFalse(entryTelemetry(entry).contains(" of "))
        val notification=downloadNotification(listOf(entry))!!
        assertTrue(notification.text.startsWith("Direct Play"))
        assertEquals(0,notification.progress)
    }
}

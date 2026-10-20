package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DetailLabelsTest {
    @Test
    fun leftOffLineUsesPlaybackTime() {
        assertEquals("Left off at 12:04", leftOffLine(724_000L))
        assertEquals("Left off at 1:00:01", leftOffLine(3_601_000L))
        assertEquals("Left off at 0:45", leftOffLine(45_000L))
    }

    @Test
    fun summaryNamesTheFileOnlyWhenSeveralVideosQualify() {
        val only = video(index = 1, name = "Only.mkv", positionMs = 724_000L, durationMs = 3_600_000L)
        assertEquals("Left off at 12:04", leftOffSummaryLine(only, qualifyingCount = 1))
        assertEquals("Left off at 12:04 · Only.mkv", leftOffSummaryLine(only, qualifyingCount = 2))
        assertEquals("Left off at 12:04", leftOffSummaryLine(only.copy(name = "  "), qualifyingCount = 3))
    }

    @Test
    fun ignoresEmptyNearEndNonVideoAndUnselectedIncomplete() {
        val idle = video(index = 0, positionMs = 0L, durationMs = 120_000L)
        val nearEnd = video(index = 1, positionMs = 117_000L, durationMs = 120_000L)
        val justInside = video(index = 2, positionMs = 116_999L, durationMs = 120_000L, bookmarkUpdatedAt = 1L)
        val notes = video(index = 3, name = "notes.txt", path = "notes.txt", positionMs = 45_000L, durationMs = 120_000L)
        val subtitle = video(index = 4, name = "movie.en.srt", path = "movie.en.srt", positionMs = 45_000L, durationMs = 120_000L)
        val hidden = video(index = 5, positionMs = 50_000L, durationMs = 120_000L, bookmarkUpdatedAt = 9_000L)
        val unknownDuration = video(index = 6, positionMs = 45_000L, durationMs = 0L, bookmarkUpdatedAt = 2L)

        assertFalse(isLeftOffCandidate(idle, selected = true))
        assertFalse(isLeftOffCandidate(nearEnd, selected = true))
        assertTrue(isLeftOffCandidate(justInside, selected = true))
        assertFalse(isLeftOffCandidate(notes, selected = true))
        assertFalse(isLeftOffCandidate(subtitle, selected = true))
        assertFalse(isLeftOffCandidate(hidden, selected = false))
        assertTrue(showsLeftOffTime(45_000L, 0L))
        assertFalse(showsPlaybackBar(45_000L, 0L))
        assertFalse(showsPlaybackBar(117_000L, 120_000L))
        assertTrue(showsPlaybackBar(116_999L, 120_000L))

        val choice = leftOffChoice(
            listOf(idle, nearEnd, justInside, notes, subtitle, hidden, unknownDuration),
            selected = setOf(0, 1, 2, 3, 4, 6),
        )
        assertEquals(6, choice?.file?.index)
        assertEquals(2, choice?.qualifyingCount)
        assertEquals("Left off at 0:45 · episode.mkv", choice?.let { leftOffSummaryLine(it.file, it.qualifyingCount) })
    }

    @Test
    fun newestBookmarkWinsThenLowerIndex() {
        val older = video(index = 0, name = "older.mkv", positionMs = 45_000L, durationMs = 120_000L, bookmarkUpdatedAt = 1_000L)
        val newer = video(index = 4, name = "newer.mkv", positionMs = 30_000L, durationMs = 120_000L, bookmarkUpdatedAt = 2_000L)
        val tieLow = video(index = 1, name = "tie-low.mkv", positionMs = 10_000L, durationMs = 120_000L, bookmarkUpdatedAt = 5_000L)
        val tieHigh = video(index = 3, name = "tie-high.mkv", path = "Season 2/tie-high.mkv", positionMs = 20_000L, durationMs = 120_000L, bookmarkUpdatedAt = 5_000L)
        val finishedNewer = newer.copy(playbackPositionMs = 118_000L, bookmarkUpdatedAt = 9_000L)

        val newest = leftOffChoice(listOf(older, newer), selected = setOf(0, 4))
        assertEquals(4, newest?.file?.index)
        assertEquals(2, newest?.qualifyingCount)
        assertEquals("Left off at 0:30 · newer.mkv", newest?.let { leftOffSummaryLine(it.file, it.qualifyingCount) })

        val tie = leftOffChoice(listOf(tieHigh, tieLow), selected = setOf(1, 3))
        assertEquals(1, tie?.file?.index)
        assertEquals("Left off at 0:10 · tie-low.mkv", tie?.let { leftOffSummaryLine(it.file, it.qualifyingCount) })

        val fallback = leftOffChoice(listOf(older, finishedNewer), selected = setOf(0, 4))
        assertEquals(0, fallback?.file?.index)
        assertEquals(1, fallback?.qualifyingCount)
        assertEquals("Left off at 0:45", fallback?.let { leftOffSummaryLine(it.file, it.qualifyingCount) })
        assertNull(leftOffChoice(listOf(finishedNewer), selected = setOf(4)))
    }

    @Test
    fun selectedOrVerifiedVideosQualifyIncludingPathExtension() {
        val selectedPartial = video(index = 2, name = "partial.mkv", positionMs = 8_000L, durationMs = 90_000L, bookmarkUpdatedAt = 10L)
        val verified = video(
            index = 7,
            name = "done.mkv",
            positionMs = 8_000L,
            durationMs = 90_000L,
            bookmarkUpdatedAt = 50L,
            length = 100L,
            verifiedBytes = 100L,
        )
        val pathVideo = video(
            index = 1,
            name = "release",
            path = "Season 01/episode.mp4",
            positionMs = 8_000L,
            durationMs = 90_000L,
            bookmarkUpdatedAt = 40L,
        )
        val choice = leftOffChoice(
            listOf(selectedPartial, verified, pathVideo),
            selected = setOf(2, 1),
        )
        assertEquals(7, choice?.file?.index)
        assertEquals(3, choice?.qualifyingCount)
        assertTrue(isLeftOffCandidate(verified, selected = false))
        assertTrue(isLeftOffCandidate(pathVideo, selected = true))
        assertFalse(isLeftOffCandidate(selectedPartial.copy(verifiedBytes = 0L), selected = false))
    }

    @Test
    fun zeroLengthVideoCountsAsVerified() {
        val empty = video(index = 0, name = "empty.mkv", positionMs = 4_000L, durationMs = 60_000L, length = 0L)
        assertTrue(empty.isVerifiedComplete)
        assertEquals(0, leftOffChoice(listOf(empty), selected = emptySet())?.file?.index)
    }

    @Test
    fun playbackFractionIsBoundedAndRequiresDuration() {
        assertEquals(0.5f, playbackFraction(50_000L, 100_000L)!!, 0.0001f)
        assertEquals(1f, playbackFraction(150_000L, 100_000L)!!, 0.0001f)
        assertEquals(0f, playbackFraction(0L, 100_000L)!!, 0.0001f)
        assertNull(playbackFraction(10_000L, 0L))
        assertNull(playbackFraction(-5L, -1L))
    }

    @Test
    fun typeLabelPrefersVideoThenSubtitleThenAudio() {
        assertEquals("Video", detailTypeLabel("movie.mkv", "movie.mkv"))
        assertEquals("Video", detailTypeLabel("release", "pack/movie.webm"))
        assertEquals("Video", detailTypeLabel("clip.mp4", "notes.txt"))
        assertEquals("Subtitle", detailTypeLabel("movie.en.srt", "subs/movie.en.srt"))
        assertEquals("Subtitle", detailTypeLabel("movie.mp4.srt", "movie.mp4.srt"))
        assertEquals("Audio", detailTypeLabel("song.flac", "song.flac"))
        assertEquals("Audio", detailTypeLabel("track.bin", "cd/track.mp3"))
        assertEquals("Other", detailTypeLabel("readme.nfo", "readme.nfo"))
    }

    @Test
    fun parentFolderShowsNestedPathOnly() {
        assertNull(parentFolderLabel("Episode.mkv", "Episode.mkv"))
        assertNull(parentFolderLabel("dir/file.mkv", "dir/file.mkv"))
        assertNull(parentFolderLabel("Episode.mkv", "/Episode.mkv"))
        assertNull(parentFolderLabel("E.mkv", "Season\\E.mkv"))
        assertEquals("Season 01", parentFolderLabel("Episode.mkv", "Season 01/Episode.mkv"))
        assertEquals("Show/Season 01", parentFolderLabel("Episode.mkv", "Show/Season 01/Episode.mkv"))
        assertEquals("audio", parentFolderLabel("song.mp3", "audio/song.mp3"))
    }

    @Test
    fun priorityBorderStaysWhenRowIsAlsoLeftOff() {
        assertEquals(DetailFileBorder.PRIORITY, detailFileBorder(priorityDownload = true, comeBack = true))
        assertEquals(DetailFileBorder.PRIORITY, detailFileBorder(priorityDownload = true, comeBack = false))
        assertEquals(DetailFileBorder.LEFT_OFF, detailFileBorder(priorityDownload = false, comeBack = true))
        assertEquals(DetailFileBorder.NEUTRAL, detailFileBorder(priorityDownload = false, comeBack = false))
    }

    private fun video(
        index: Int,
        name: String = "episode.mkv",
        path: String = name,
        positionMs: Long = 0L,
        durationMs: Long = 0L,
        bookmarkUpdatedAt: Long = 0L,
        length: Long = 1_000L,
        verifiedBytes: Long = 0L,
    ) = SavedFile(
        index = index,
        name = name,
        path = path,
        length = length,
        verifiedBytes = verifiedBytes,
        playbackPositionMs = positionMs,
        playbackDurationMs = durationMs,
        bookmarkUpdatedAt = bookmarkUpdatedAt,
    )
}

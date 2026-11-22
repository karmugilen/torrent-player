package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.PieceBucket
import webtor.core.TorrentFile

class SeekReadinessTest {
    private val mb = 1L shl 20
    private fun file(index: Int, length: Long, progress: Double = 0.5) =
        TorrentFile(index, "f$index", "f$index", length, progress, "video")
    private fun bucket(start: Int, end: Int, verified: Int) =
        PieceBucket(start, end, total = end - start + 1, selected = end - start + 1, verified = verified, receiving = 0)

    /** Second file of 100 MB after a 20 MB one, 10 MB pieces: the video is pieces 2..11. */
    private fun twoFiles(vararg buckets: PieceBucket) = SeekReadiness().apply {
        assertTrue(file("t", 1, listOf(file(1, 100 * mb), file(0, 20 * mb))))
        pieces(10 * mb, buckets.toList())
    }

    @Test fun unknownBeforeAnyPieceMap() {
        val r = SeekReadiness()
        assertNull(r.readyAt(0.5f))
        assertNull(r.aimAt(30.0, 60.0))
    }

    @Test fun spansAreClippedToThePlayingFile() {
        val r = twoFiles(bucket(0, 3, 4), bucket(4, 11, 0))
        assertEquals(listOf(SeekReadiness.Span(0f, 0.2f, 1f), SeekReadiness.Span(0.2f, 1f, 0f)), r.spans)
    }

    @Test fun verifiedStretchIsReadyOthersAreNot() {
        val r = twoFiles(bucket(0, 3, 4), bucket(4, 11, 0))
        assertEquals(true, r.readyAt(0.1f))
        assertEquals(false, r.readyAt(0.5f))
        assertEquals(true, r.readyAt(6.0, 60.0))
        assertNull(r.readyAt(6.0, 0.0))
    }

    @Test fun ninetyNinePercentCountsAsReady() {
        val r = SeekReadiness().apply {
            file("t", 0, listOf(file(0, 1000 * mb)))
            pieces(mb, listOf(bucket(0, 999, 990)))
        }
        assertEquals(true, r.readyAt(0.5f))
        r.pieces(mb, listOf(bucket(0, 999, 989)))
        assertEquals(false, r.readyAt(0.5f))
    }

    @Test fun finishedFileIsReadyEverywhereWithoutAPieceMap() {
        val r = SeekReadiness()
        assertFalse(r.file("t", 0, listOf(file(0, 50 * mb, progress = 1.0))))
        assertEquals(listOf(SeekReadiness.Span(0f, 1f, 1f)), r.spans)
        assertEquals(true, r.readyAt(0.99f))
    }

    @Test fun missingOrEmptyFileChangesNothing() {
        val r = SeekReadiness()
        assertFalse(r.file("t", 3, listOf(file(0, 50 * mb))))
        assertFalse(r.file("t", 0, listOf(file(0, 0))))
        assertNull(r.aimAt(10.0, 60.0))
    }

    @Test fun aimFetchesFromTwoMegabytesBeforeTheSpotByAverageBitrate() {
        val r = twoFiles(bucket(0, 3, 4), bucket(4, 11, 0))
        assertEquals(SeekReadiness.Aim("t", 1, 50 * mb - 2 * mb, 10 * mb), r.aimAt(30.0, 60.0))
    }

    @Test fun noAimWhenTheSpotIsReady() {
        val r = twoFiles(bucket(0, 3, 4), bucket(4, 11, 0))
        assertNull(r.aimAt(6.0, 60.0))
        assertNull(r.aimAt(30.0, 0.0))
    }

    @Test fun clearForgetsTheFile() {
        val r = twoFiles(bucket(0, 11, 0))
        r.clear()
        assertNull(r.readyAt(0.5f))
        assertNull(r.aimAt(30.0, 60.0))
    }

    @Test fun streamOfReadsEngineUrls() {
        assertEquals("abc" to 2, SeekReadiness.streamOf("http", listOf("torrent", "abc", "file", "2")))
        assertEquals("abc" to 0, SeekReadiness.streamOf("http", listOf("x", "torrent", "abc", "file", "0", "name.mkv")))
        assertNull(SeekReadiness.streamOf("content", listOf("torrent", "abc", "file", "2")))
        assertNull(SeekReadiness.streamOf("http", listOf("torrent", "abc", "files", "2")))
        assertNull(SeekReadiness.streamOf("http", listOf("torrent", "abc", "file", "two")))
        assertNull(SeekReadiness.streamOf("http", listOf("torrent", "abc", "file")))
    }
}

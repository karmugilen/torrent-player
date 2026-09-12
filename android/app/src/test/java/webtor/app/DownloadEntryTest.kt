package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DownloadEntryTest {
    private fun file(index: Int, length: Long, progress: Double, name: String = "f$index.bin") =
        SavedFile(index, name, name, length, uri = null, progress = progress)

    private fun entry(files: List<SavedFile>, selected: Set<Int>) =
        DownloadEntry("k", "title", "magnet:x", "", null, "Downloads", files, selected)

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
    fun emptySelectionIsCompleteWithFullProgress() {
        val e = entry(listOf(file(0, 100, 0.0)), emptySet())
        assertEquals(0L, e.total)
        assertEquals(0L, e.downloaded)
        assertEquals(1f, e.progress)
        assertTrue(e.complete)
    }

    @Test
    fun darkThemeIsOnByDefault() {
        assertTrue(UiState().darkTheme)
    }

    @Test
    fun findPlayerMatchesPackageAndTreatsBlankAsAsk() {
        val vlc = PlayerApp("org.videolan.vlc", "org.videolan.vlc.gui.video.VideoPlayerActivity", "VLC")
        val mpv = PlayerApp("is.xyz.mpv", "is.xyz.mpv.MPVActivity", "mpv")
        val players = listOf(vlc, mpv)
        assertEquals(vlc, findPlayer(players, "org.videolan.vlc"))
        assertEquals(null, findPlayer(players, ""))
        assertEquals(null, findPlayer(players, "com.missing.player"))
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
}

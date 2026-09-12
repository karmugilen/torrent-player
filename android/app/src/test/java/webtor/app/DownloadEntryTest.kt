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

    @Test
    fun completedPlaybackUsesSelectedVideoWithoutEngine() {
        val e = entry(listOf(
            file(0, 100, 1.0, "movie.mp4").copy(uri = "content://media/1"),
            file(1, 200, 1.0, "notes.txt").copy(uri = "content://media/2"),
        ), setOf(0, 1))
        assertEquals(0, completedPlayFile(e)?.index)
        assertEquals(null, completedPlayFile(e.copy(files = e.files.map { it.copy(progress = 0.5) })))
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
        assertEquals("Finding peers…", prefetchStatus(draft))
        assertEquals("Finding peers…", prefetchStatus(draft.copy(torrent = null)))
        assertEquals("Connecting · 2 peers", prefetchStatus(draft.copy(torrent = t.copy(numPeers = 2))))
        assertTrue(prefetchStatus(draft.copy(torrent = t.copy(downloaded = 10)))!!.contains("already downloading"))
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
    fun explicitPlaybackChoosesRequestedEpisode() {
        val e = entry(listOf(
            file(0, 200, 1.0, "episode-1.mp4").copy(uri = "content://media/1"),
            file(1, 100, 1.0, "episode-2.mp4").copy(uri = "content://media/2"),
            file(2, 100, 1.0, "unselected.mp4").copy(uri = "content://media/3"),
        ), setOf(0, 1))
        assertEquals("content://media/2", completedPlayFile(e, 1)?.uri)
        assertEquals("content://media/1", completedPlayFile(e, 0)?.uri)
        assertEquals(null, completedPlayFile(e, 2))
        assertEquals(null, completedPlayFile(e, 99))
    }
}

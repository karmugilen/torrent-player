package webtor.app

import java.net.URL
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class TorrentFetchTest {
    private val magnet = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789"

    @Test fun findsTheLinkInsideSharedText() {
        assertEquals(magnet, extractTorrentLink("Big Buck Bunny\n$magnet"))
        assertEquals("https://example.org/a.torrent", extractTorrentLink("Get it at https://example.org/a.torrent."))
        assertEquals(magnet, extractTorrentLink("https://example.org/page and $magnet"))
        assertNull(extractTorrentLink("no link here"))
        assertNull(extractTorrentLink(null))
    }

    @Test fun recognisesBencodedTorrentsOnly() {
        assertTrue(looksLikeTorrentFile("d8:announce20:http://example.org/a4:infod4:name1:aee".toByteArray()))
        assertFalse(looksLikeTorrentFile("<!DOCTYPE html><html><body>Not found</body></html>".toByteArray()))
        assertFalse(looksLikeTorrentFile(ByteArray(0)))
    }

    @Test fun plainHttpLinksAreAskedForOverHttps() {
        assertEquals("https://example.org/a.torrent?x=1", secureTorrentUrl(URL("http://example.org/a.torrent?x=1")).toString())
        assertEquals("https://example.org:8443/a.torrent", secureTorrentUrl(URL("https://example.org:8443/a.torrent")).toString())
        assertThrows(IllegalStateException::class.java) { secureTorrentUrl(URL("http://example.org:8080/a.torrent")) }
    }

    @Test fun fileSourcesKeepTheFileName() {
        val source = torrentFileSource("Sintel.torrent")
        assertTrue(isTorrentFileSource(source))
        assertEquals("Sintel.torrent", torrentFileName(source))
        assertEquals("Torrent file", torrentFileName(torrentFileSource(" ")))
        assertFalse(isTorrentFileSource(magnet))
    }

    @Test fun oversizedTorrentsAreRejected() {
        assertThrows(IllegalStateException::class.java) {
            readTorrentBytes(ByteArray(MAX_TORRENT_BYTES + 1).inputStream())
        }
    }
}

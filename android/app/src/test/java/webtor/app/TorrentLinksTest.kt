package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TorrentLinksTest {
    @Test fun trimsSharedLinksAndAcceptsQueryStrings() {
        val magnet = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789"
        assertEquals(magnet, supportedTorrentLink("  $magnet\n"))
        assertEquals("https://example.org/file.torrent?token=abc", supportedTorrentLink("https://example.org/file.torrent?token=abc"))
    }

    @Test fun rejectsPrivatePathsAndDeceptiveSchemes() {
        listOf("file:///data/user/0/webtor.app/shared_prefs/settings.xml", "/tmp/test.torrent",
            "http-evil://example.org/file", "https:///file.torrent", "https://user:secret@example.org/file",
            "magnet:?dn=missing-info-hash", "magnet:?xt=urn:btih:short", "https://example.org/file with spaces", "").forEach {
            assertNull(it, supportedTorrentLink(it))
        }
    }

    @Test fun acceptsBase32AndHexInfoHashes() {
        assertEquals(
            "magnet:?xt=urn:btih:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567",
            supportedTorrentLink("magnet:?xt=urn:btih:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"),
        )
    }
}

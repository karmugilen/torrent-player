package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
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

    @Test fun acceptsPureBt2BtmhMagnets() {
        val pureV2 =
            "magnet:?xt=urn:btmh:1220caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e&dn=bittorrent-v2-test"
        assertEquals(pureV2, supportedTorrentLink(pureV2))
        assertEquals(pureV2, supportedTorrentLink("  $pureV2\n"))
        assertNull(supportedTorrentLink("magnet:?xt=urn:btmh:1220dead")) // too short
        assertNull(supportedTorrentLink("magnet:?xt=urn:btmh:1320" + "ab".repeat(32))) // not SHA-256 multihash
        assertEquals(
            "caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa",
            infoHashFromMagnet(pureV2),
        )
    }

    @Test fun acceptsHybridBtihAndBtmh() {
        val hybrid =
            "magnet:?xt=urn:btih:631a31dd0a46257d5078c0dee4e66e26f73e42ac&xt=urn:btmh:1220d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb&dn=bittorrent-v1-v2-hybrid-test"
        assertEquals(hybrid, supportedTorrentLink(hybrid))
    }

    @Test fun normalizeTorrentIdLocalMatchesLibraryHexKeys() {
        val hex = "51340689c960f0778a4387aef9b4b52fd08390cd"
        assertEquals(hex to hex, normalizeTorrentIdLocal(hex.uppercase()))
        val base32 = "ZOCMZQIPFFW7OLLMIC5HUB6BPCSDEOQU"
        val normalized = normalizeTorrentIdLocal(base32)
        assertEquals(40, normalized.first.length)
        assertEquals(normalized.first, normalized.second)
        val magnet = normalizeTorrentIdLocal("magnet:?xt=urn:btih:$base32")
        assertEquals("magnet:?xt=urn:btih:$base32", magnet.first)
        assertEquals(normalized.second, magnet.second)
    }

    @Test fun keyboardHidesOnlyAfterACompleteLinkIsPasted() {
        val magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
        assertTrue(shouldHideKeyboardAfterPaste("", magnet))
        assertFalse(shouldHideKeyboardAfterPaste(magnet.dropLast(1), magnet))
        assertFalse(shouldHideKeyboardAfterPaste(magnet, magnet))
        assertFalse(shouldHideKeyboardAfterPaste("", "magnet:?xt=urn:btih:short"))
    }
}

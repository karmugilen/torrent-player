package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.TorrentStatus

class MagnetShareTest {

    private fun testEntry(
        key: String = "test-key",
        source: String = "content://downloads/sample.torrent",
        status: TorrentStatus? = null,
    ) = DownloadEntry(
        key = key,
        title = "Test Torrent",
        source = source,
        metadata = "",
        engineId = null,
        destination = "Downloads",
        files = emptyList(),
        selected = emptySet(),
        status = status,
    )

    private fun testStatus(
        magnetURI: String? = null,
        infoHash: String? = null,
    ) = TorrentStatus(
        id = "engine-id-1",
        infoHash = infoHash,
        name = "Test Torrent",
        magnetURI = magnetURI,
        ready = true,
        done = false,
        paused = false,
        progress = 0.5,
        downloadSpeed = 1000L,
        uploadSpeed = 500L,
        numPeers = 5,
        length = 10000L,
        downloaded = 5000L,
        uploaded = 2500L,
        files = emptyList(),
    )

    @Test
    fun prefersStatusMagnetUriOverSourceAndInfoHash() {
        val entry = testEntry(
            key = "0123456789abcdef0123456789abcdef01234567",
            source = "magnet:?xt=urn:btih:sourcehash",
            status = testStatus(
                magnetURI = "magnet:?xt=urn:btih:statushash&tr=http://tracker.com",
                infoHash = "statushash",
            ),
        )
        assertEquals("magnet:?xt=urn:btih:statushash&tr=http://tracker.com", resolveMagnetUri(entry))
    }

    @Test
    fun ignoresBlankStatusMagnetUriAndFallsBackToSource() {
        val entry = testEntry(
            key = "0123456789abcdef0123456789abcdef01234567",
            source = "magnet:?xt=urn:btih:sourcehash",
            status = testStatus(
                magnetURI = "   ",
                infoHash = "0123456789abcdef0123456789abcdef01234567",
            ),
        )
        assertEquals("magnet:?xt=urn:btih:sourcehash", resolveMagnetUri(entry))
    }

    @Test
    fun usesSourceWhenItLooksLikeMagnet() {
        val entry = testEntry(
            key = "torrent-uuid-1234",
            source = "MAGNET:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12&dn=Movie",
            status = null,
        )
        assertEquals(
            "MAGNET:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12&dn=Movie",
            resolveMagnetUri(entry),
        )
    }

    @Test
    fun buildsMagnetFromKeyWhenSourceIsNotMagnet() {
        val hash = "0123456789abcdef0123456789abcdef01234567"
        val entry = testEntry(
            key = hash,
            source = "file:///storage/sample.torrent",
            status = null,
        )
        assertEquals("magnet:?xt=urn:btih:$hash", resolveMagnetUri(entry))
    }

    @Test
    fun buildsMagnetFromBase32Key() {
        val base32Hash = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
        val entry = testEntry(
            key = base32Hash,
            source = "https://example.com/file.torrent",
            status = null,
        )
        assertEquals("magnet:?xt=urn:btih:$base32Hash", resolveMagnetUri(entry))
    }

    @Test
    fun buildsMagnetFromStatusInfoHashWhenKeyIsNotHash() {
        val hash = "fedcba9876543210fedcba9876543210fedcba98"
        val entry = testEntry(
            key = "engine-session-123",
            source = "content://com.android.providers/123",
            status = testStatus(infoHash = hash),
        )
        assertEquals("magnet:?xt=urn:btih:$hash", resolveMagnetUri(entry))
    }

    @Test
    fun returnsNullWhenNoMagnetOrInfoHashAvailable() {
        val entry = testEntry(
            key = "engine-session-123",
            source = "content://com.android.providers/123",
            status = testStatus(infoHash = null),
        )
        assertNull(resolveMagnetUri(entry))
    }

    @Test
    fun isLikelyInfoHashValidation() {
        assertTrue(isLikelyInfoHash("0123456789abcdef0123456789abcdef01234567"))
        assertTrue(isLikelyInfoHash("0123456789ABCDEF0123456789ABCDEF01234567"))
        assertTrue(isLikelyInfoHash("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"))
        assertFalse(isLikelyInfoHash(null))
        assertFalse(isLikelyInfoHash(""))
        assertFalse(isLikelyInfoHash("   "))
        assertFalse(isLikelyInfoHash("short"))
        assertFalse(isLikelyInfoHash("0123456789abcdef0123456789abcdef0123456g")) // 'g' is not hex
        assertFalse(isLikelyInfoHash("engine-session-123"))
    }
}

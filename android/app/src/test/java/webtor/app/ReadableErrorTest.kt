package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ReadableErrorTest {
    private val engineDown = "The download engine is not running. Close Torrent Player and open it again."

    @Test
    fun networkRefusalsAreNotReportedAsAStoppedEngine() {
        listOf(
            "Get https://example.org/a.torrent: dial tcp 93.184.216.34:443: connect: connection refused",
            "failed to connect to tracker",
            "dial tcp: lookup example.org: no such host",
            "read udp 10.0.0.2:5000: i/o timeout",
            "connect: network is unreachable",
        ).forEach { message ->
            val shown = readableError(message)
            assertTrue(message, shown.startsWith("Couldn't connect on this network."))
        }
    }

    @Test
    fun engineStartFailuresStillSayTheEngineIsDown() {
        assertEquals(engineDown, readableError("engine did not start"))
        assertEquals(engineDown, readableError("Engine down"))
        assertEquals(engineDown, readableError("Native engine initialization failed"))
    }

    @Test
    fun pausedTorrentDataExplainsTheCause() {
        val shown = readableError("HTTP 409: {\"error\":\"payload transfers are disabled in settings\"}")
        assertTrue(shown, shown.startsWith("Torrent data is paused by low battery or the network setting."))
    }
}

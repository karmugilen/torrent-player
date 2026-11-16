package webtor.app

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.EngineClient
import webtor.core.EngineResponse
import webtor.core.HolepunchStats

class HolepunchParsingTest {

    private fun clientReturning(body: String) = EngineClient { _, _, _ -> EngineResponse(200, body) }

    @Test
    fun statsReadsAllSixHolepunchCounters() {
        val stats = clientReturning(
            """{"downloadSpeed":0,"listenPort":6881,"holepunch":{"rendezvousSent":7,"connectReceived":5,
               "relayedForOthers":3,"errorsReceived":2,"punchDialsAttempted":4,"punchDialsSucceeded":1}}""",
        ).stats()
        assertEquals(HolepunchStats(7, 5, 3, 2, 4, 1), stats.holepunch)
        assertEquals(6881, stats.listenPort)
    }

    @Test
    fun missingHolepunchObjectReadsAsZeros() {
        val stats = clientReturning("""{"downloadSpeed":0,"listenPort":6881}""").stats()
        assertEquals(HolepunchStats(), stats.holepunch)
        assertTrue(stats.holepunch.rows().all { it.second == 0L })
    }

    @Test
    fun partialOrNullHolepunchObjectFallsBackToZeros() {
        assertEquals(HolepunchStats(), EngineClient.parseHolepunchStats(null))
        val partial = EngineClient.parseHolepunchStats(JSONObject("""{"punchDialsAttempted":9}"""))
        assertEquals(HolepunchStats(punchDialsAttempted = 9), partial)
        assertEquals(HolepunchStats(), EngineClient.parseHolepunchStats(JSONObject("""{"rendezvousSent":null}""")))
    }

    @Test
    fun counterRowsKeepEngineOrder() {
        val rows = HolepunchStats(1, 2, 3, 4, 5, 6).rows()
        assertEquals(6, rows.size)
        assertEquals(listOf(1L, 2L, 3L, 4L, 5L, 6L), rows.map { it.second })
    }

    @Test
    fun peerSourceMarksHolePunchedPeersOnly() {
        val connections = clientReturning(
            """{"id":"t1","paused":false,"peers":[
                {"address":"1.2.3.4:5000","transport":"uTP","client":"qB","downloaded":1,"uploaded":0,"source":"C"},
                {"address":"5.6.7.8:6000","transport":"TCP","client":"Tr","downloaded":0,"uploaded":0,"source":"Tr"},
                {"address":"9.9.9.9:7000","transport":"TCP","client":"old","downloaded":0,"uploaded":0}
            ]}""",
        ).connections("t1")
        val (punched, tracker, legacy) = connections.peers
        assertEquals("C", punched.source)
        assertTrue(punched.holePunched)
        assertEquals("Tr", tracker.source)
        assertFalse(tracker.holePunched)
        assertNull(legacy.source)
        assertFalse(legacy.holePunched)
    }

    @Test
    fun peerBittorrentVersionMarksBt1AndBt2() {
        val connections = clientReturning(
            """{"id":"t1","paused":false,"peers":[
                {"address":"1.2.3.4:5000","transport":"TCP","client":"qB","downloaded":1,"uploaded":0,"bittorrentVersion":"v1"},
                {"address":"5.6.7.8:6000","transport":"uTP","client":"lt","downloaded":0,"uploaded":0,"bittorrentVersion":"v2","source":"C"},
                {"address":"9.9.9.9:7000","transport":"TCP","client":"old","downloaded":0,"uploaded":0}
            ]}""",
        ).connections("t1")
        val (v1, v2, legacy) = connections.peers
        assertEquals("v1", v1.bittorrentVersion)
        assertFalse(v1.isBt2)
        assertEquals("v2", v2.bittorrentVersion)
        assertTrue(v2.isBt2)
        assertTrue(v2.holePunched)
        assertNull(legacy.bittorrentVersion)
        assertFalse(legacy.isBt2)
        assertEquals(2, connections.peers.count { !it.isBt2 })
        assertEquals(1, connections.peers.count { it.isBt2 })
    }

    @Test
    fun connectionsReadsPerTorrentHolepunchCounters() {
        val connections = clientReturning(
            """{"id":"t1","paused":false,"peers":[],"holepunch":{"rendezvousSent":2,"connectReceived":1,
               "relayedForOthers":0,"errorsReceived":0,"punchDialsAttempted":1,"punchDialsSucceeded":1}}""",
        ).connections("t1")
        assertEquals(HolepunchStats(2, 1, 0, 0, 1, 1), connections.holepunch)
    }

    /** An engine without per-torrent counters gives null, never a zeroed or client-wide stand-in. */
    @Test
    fun connectionsWithoutHolepunchObjectIsNull() {
        val connections = clientReturning("""{"id":"t1","paused":false,"peers":[]}""").connections("t1")
        assertNull(connections.holepunch)
    }

    @Test
    fun connectionsPartialHolepunchObjectFillsZeros() {
        val connections = clientReturning(
            """{"id":"t1","paused":false,"peers":[],"holepunch":{"punchDialsSucceeded":3}}""",
        ).connections("t1")
        assertEquals(HolepunchStats(punchDialsSucceeded = 3), connections.holepunch)
    }
}

package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import webtor.core.EngineClient
import webtor.core.EngineResponse
import webtor.core.PeerConnection

class SwarmSummaryTest {

    private fun peer(version: String? = null, source: String? = null) =
        PeerConnection("1.2.3.4:5000", "TCP", "qB", 0, 0, source = source, bittorrentVersion = version)

    @Test fun fullLineFromCurrentEngine() {
        val peers = listOf(peer("v1", "Tr"), peer("v1", "C"), peer("v2", "C"), peer("v2", "X"))
        assertEquals("4 peers · BT1 2 · BT2 2 · 2 hole-punched", swarmPeerSummary(peers))
    }

    @Test fun zeroCountsAreShownWhenTheEngineReportsTheFields() {
        assertEquals("2 peers · BT1 2 · BT2 0 · 0 hole-punched", swarmPeerSummary(listOf(peer("v1", "Tr"), peer("v1", "X"))))
        assertEquals("1 peer · BT1 0 · BT2 1 · 0 hole-punched", swarmPeerSummary(listOf(peer("v2", "Tr"))))
    }

    /** An older engine sends neither field: do not claim "BT1 N · BT2 0" or "0 hole-punched". */
    @Test fun olderEngineWithoutFieldsShowsOnlyPeerCount() {
        assertEquals("3 peers", swarmPeerSummary(listOf(peer(), peer(), peer())))
    }

    @Test fun unknownVersionIsNotCountedAsBt1() {
        assertEquals("2 peers · BT1 1 · BT2 0", swarmPeerSummary(listOf(peer("v1"), peer())))
    }

    @Test fun webSeedsAreNotCountedAsBt1OrBt2() {
        val seed = PeerConnection("http://example.org/f", "WebSeed", "", 0, 0, source = "WS", bittorrentVersion = "v1")
        assertEquals("3 peers · BT1 1 · BT2 1 · 1 hole-punched", swarmPeerSummary(listOf(peer("v1", "C"), peer("v2", "Tr"), seed)))
        assertEquals("1 peer · 0 hole-punched", swarmPeerSummary(listOf(seed)))
    }

    /** End to end from the JSON names the engine emits (wt-engine PeerConnectionView). */
    @Test fun parsesEngineJsonIntoSummary() {
        val client = EngineClient { _, _, _ ->
            EngineResponse(200, """{"id":"t","paused":false,"peers":[
                {"address":"1.1.1.1:1","transport":"uTP","client":"a","downloaded":0,"uploaded":0,"source":"C","bittorrentVersion":"v2"},
                {"address":"2.2.2.2:2","transport":"TCP","client":"b","downloaded":0,"uploaded":0,"source":"Hg","bittorrentVersion":"v1"}
            ]}""")
        }
        assertEquals("2 peers · BT1 1 · BT2 1 · 1 hole-punched", swarmPeerSummary(client.connections("t").peers))
    }

    /** The block's "Hole-punched now" row and the swarm line count the same peers of the same torrent. */
    @Test fun holepunchBlockMatchesSwarmLine() {
        val client = EngineClient { _, _, _ ->
            EngineResponse(200, """{"id":"t","paused":false,"peers":[
                {"address":"1.1.1.1:1","transport":"uTP","client":"a","downloaded":0,"uploaded":0,"source":"C","bittorrentVersion":"v2"},
                {"address":"3.3.3.3:3","transport":"uTP","client":"c","downloaded":0,"uploaded":0,"source":"C","bittorrentVersion":"v1"},
                {"address":"2.2.2.2:2","transport":"TCP","client":"b","downloaded":0,"uploaded":0,"source":"Hg","bittorrentVersion":"v1"}
            ],"holepunch":{"rendezvousSent":4,"connectReceived":3,"relayedForOthers":1,"errorsReceived":0,
                "punchDialsAttempted":3,"punchDialsSucceeded":2}}""")
        }
        val snap = client.connections("t")
        assertEquals("3 peers · BT1 2 · BT2 1 · 2 hole-punched", swarmPeerSummary(snap.peers))
        val rows = holepunchRows(snap.holepunch!!, snap.peers)
        assertEquals("Hole-punched now" to 2L, rows.first())
        assertEquals(7, rows.size)
        assertEquals("Punched peers" to 2L, rows.last())
        assertEquals(holePunchedNow(snap.peers).toLong(), rows.first().second)
    }

    // ---- pure-v2 library key

    @Test fun normalizedPureV2HashMapsToEngineTorrentKey() {
        val full = "caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa9f6105232b28ad099f3a302e"
        val short = "caf1e1c30e81cb361b9ee167c4aa64228a7fa4fa"
        assertEquals(short, libraryKeyForInfoHash(full))
        assertEquals(short, libraryKeyForInfoHash(full.uppercase()))
        val magnet = "magnet:?xt=urn:btmh:1220$full&dn=bittorrent-v2-test"
        assertEquals(libraryKeyForInfoHash(full), infoHashFromMagnet(magnet))
        assertEquals(short, normalizeTorrentIdLocal(magnet).second)
    }

    @Test fun v1KeysAreUnchanged() {
        val v1 = "631a31dd0a46257d5078c0dee4e66e26f73e42ac"
        assertEquals(v1, libraryKeyForInfoHash(v1.uppercase()))
        assertNull(libraryKeyForInfoHash(null))
        assertNull(libraryKeyForInfoHash("  "))
    }

    @Test fun hybridMagnetKeysByV1AndBase32ByHex() {
        val hybrid = "magnet:?xt=urn:btih:631a31dd0a46257d5078c0dee4e66e26f73e42ac&xt=urn:btmh:1220d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb"
        assertEquals("631a31dd0a46257d5078c0dee4e66e26f73e42ac", infoHashFromMagnet(hybrid))
        // btmh before btih in the URI still keys by v1, as the engine does.
        val reversed = "magnet:?xt=urn:btmh:1220d8dd32ac93357c368556af3ac1d95c9d76bd0dff6fa9833ecdac3d53134efabb&xt=urn:btih:631a31dd0a46257d5078c0dee4e66e26f73e42ac"
        assertEquals("631a31dd0a46257d5078c0dee4e66e26f73e42ac", infoHashFromMagnet(reversed))
        assertEquals(reversed, supportedTorrentLink(reversed))
        // Base32 btih → hex.
        assertEquals(
            "51340689c960f0778a4387aef9b4b52fd08390cd",
            infoHashFromMagnet("magnet:?xt=urn:btih:KE2ANCOJMDYHPCSDQ6XPTNFVF7IIHEGN"),
        )
    }
}

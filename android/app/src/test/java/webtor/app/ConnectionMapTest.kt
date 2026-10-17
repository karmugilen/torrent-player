package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.PeerConnection

class ConnectionMapTest {

    @Test
    fun peerConnectionKeyCombinesTransportAndAddress() {
        val peer = PeerConnection(address = "192.168.1.1:6881", transport = "TCP", client = "qB/4.5", downloaded = 100, uploaded = 50)
        assertEquals("TCP/192.168.1.1:6881", peer.connectionKey())
    }

    @Test
    fun activityDetectionIdentifiesDownloadingAndUploadingDeltas() {
        val peerKey = "TCP/1.2.3.4:5000"
        val previous = mapOf(peerKey to PeerConnection("1.2.3.4:5000", "TCP", "libtorrent", 1000L, 500L))

        // When downloaded increases, activity is true (receiving)
        val receivingPeer = PeerConnection("1.2.3.4:5000", "TCP", "libtorrent", 1500L, 500L)
        val recActivity = buildMap {
            val before = previous[receivingPeer.connectionKey()] ?: return@buildMap
            if (receivingPeer.downloaded > before.downloaded) put(receivingPeer.connectionKey(), true)
            else if (receivingPeer.uploaded > before.uploaded) put(receivingPeer.connectionKey(), false)
        }
        assertEquals(true, recActivity[peerKey])

        // When uploaded increases, activity is false (sending)
        val sendingPeer = PeerConnection("1.2.3.4:5000", "TCP", "libtorrent", 1000L, 800L)
        val sendActivity = buildMap {
            val before = previous[sendingPeer.connectionKey()] ?: return@buildMap
            if (sendingPeer.downloaded > before.downloaded) put(sendingPeer.connectionKey(), true)
            else if (sendingPeer.uploaded > before.uploaded) put(sendingPeer.connectionKey(), false)
        }
        assertEquals(false, sendActivity[peerKey])

        // When idle (counters unchanged), no activity entry is generated
        val idlePeer = PeerConnection("1.2.3.4:5000", "TCP", "libtorrent", 1000L, 500L)
        val idleActivity = buildMap {
            val before = previous[idlePeer.connectionKey()] ?: return@buildMap
            if (idlePeer.downloaded > before.downloaded) put(idlePeer.connectionKey(), true)
            else if (idlePeer.uploaded > before.uploaded) put(idlePeer.connectionKey(), false)
        }
        assertTrue(idleActivity.isEmpty())
    }

    @Test
    fun protocolBranchGroupingMaintainsDeterministicOrdering() {
        val p1 = PeerConnection("10.0.0.3:5000", "uTP", "client-c", 0, 0)
        val p2 = PeerConnection("10.0.0.1:5000", "TCP", "client-a", 0, 0)
        val p3 = PeerConnection("10.0.0.2:5000", "TCP", "client-b", 0, 0)
        val p4 = PeerConnection("10.0.0.4:5000", "WebRTC", "browser", 0, 0)

        val groups = listOf(p1, p2, p3, p4)
            .groupBy { it.transport }
            .toSortedMap()
            .mapValues { (_, list) ->
                list.sortedWith(compareBy({ it.address }, { it.client }))
            }

        assertEquals(listOf("TCP", "WebRTC", "uTP"), groups.keys.toList())
        assertEquals(listOf("10.0.0.1:5000", "10.0.0.2:5000"), groups["TCP"]!!.map { it.address })
    }
}

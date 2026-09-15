package webtor.core

import org.json.JSONObject
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows

private class TestResponse {
    var statusCode: Int = 200
    var body: String = ""

    fun setResponseCode(value: Int) = apply { statusCode = value }
    fun setBody(value: String) = apply { body = value }
}

private class RecordedBody(private val value: String) {
    fun readUtf8(): String = value
}

private data class RecordedRequest(
    val path: String,
    val body: RecordedBody,
)

private class FakeEngineTransport : EngineTransport {
    private val responses = ArrayDeque<TestResponse>()
    private val requests = ArrayDeque<RecordedRequest>()

    fun enqueue(response: TestResponse) { responses.addLast(response) }
    fun takeRequest(): RecordedRequest = requests.removeFirst()

    override fun request(method: String, path: String, body: String?): EngineResponse {
        requests.addLast(RecordedRequest(path, RecordedBody(body.orEmpty())))
        val response = responses.removeFirst()
        return EngineResponse(response.statusCode, response.body)
    }
}

class EngineClientTest {
    private lateinit var transport: FakeEngineTransport
    private lateinit var client: EngineClient

    @BeforeEach
    fun setUp() {
        transport = FakeEngineTransport()
        client = EngineClient(transport)
    }

    @Test
    fun statsParsesPorts() {
        transport.enqueue(
            TestResponse().setBody(
                """{"downloadSpeed":1,"uploadSpeed":2,"progress":0.5,"ratio":1.2,
                    "torrents":1,"ctlPort":0,"streamPort":8000,"path":"/tmp"}"""
            )
        )
        val s = client.stats()
        assertEquals(0, s.ctlPort)
        assertEquals(8000, s.streamPort)
        assertEquals(1L, s.downloadSpeed)
    }

    @Test
    fun addAndPlay() {
        transport.enqueue(TestResponse().setBody("""{"id":"abc","infoHash":"dead"}"""))
        val added = client.add("magnet:?xt=urn:btih:dead")
        assertEquals("abc", added.id)

        transport.enqueue(
            TestResponse().setBody(
                """{"id":"abc","fileIndex":0,"name":"a.mp4","length":12,
                    "streamUrl":"http://127.0.0.1:8000/webtorrent/dead/a.mp4"}"""
            )
        )
        val play = client.play("abc")
        assertTrue(play.streamUrl.startsWith("http://127.0.0.1:"))
        assertEquals("/add", transport.takeRequest().path)
        assertEquals("/play", transport.takeRequest().path)
    }

    @Test
    fun notFoundThrows() {
        transport.enqueue(TestResponse().setResponseCode(404).setBody("""{"error":"torrent not found"}"""))
        val ex = assertThrows<EngineException> { client.torrent("nope") }
        assertTrue(ex.message!!.contains("404"))
    }

    @Test
    fun torrentParsesStatusFields() {
        transport.enqueue(
            TestResponse().setBody(
                """{"id":"abc","infoHash":"dead","name":"n","ready":true,"done":false,
                    "paused":true,"progress":0.25,"downloadSpeed":10,"uploadSpeed":2,
                    "numPeers":3,"length":100,"downloaded":25,"uploaded":4,
                    "timeRemaining":90000,"configured":true,"selected":[0,2],
                    "files":[{"index":0,"name":"a.mp4","path":"a.mp4","length":50,"progress":0.5,"type":"video/mp4"}],
                    "error":"slow"}"""
            )
        )
        val t = client.torrent("abc")
        assertEquals(90000L, t.timeRemaining)
        assertTrue(t.configured)
        assertEquals(listOf(0, 2), t.selected)
        assertTrue(t.paused)
        assertEquals("slow", t.error)
        assertEquals("/torrent/abc", transport.takeRequest().path)
    }

    @Test
    fun progressClampsAndRejectsNonFiniteValues() {
        transport.enqueue(
            TestResponse().setBody(
                """{"id":"abc","progress":"NaN","files":[{"index":0,"name":"a.bin","path":"a.bin","length":10,"progress":2}]}"""
            )
        )
        val t = client.torrent("abc")
        assertEquals(0.0, t.progress)
        assertEquals(1.0, t.files[0].progress)
    }

    @Test
    fun torrentDefaultsMissingFields() {
        transport.enqueue(TestResponse().setBody("""{"id":"abc"}"""))
        val t = client.torrent("abc")
        assertNull(t.timeRemaining)
        assertFalse(t.configured)
        assertEquals(emptyList<Int>(), t.selected)
        assertFalse(t.paused)
        assertNull(t.error)
    }

    @Test
    fun addPrepareSendsFlag() {
        transport.enqueue(TestResponse().setBody("""{"id":"abc","infoHash":"dead"}"""))
        client.add("magnet:?xt=urn:btih:dead", prepare = true)
        val req = transport.takeRequest()
        assertEquals("/add", req.path)
        val body = JSONObject(req.body.readUtf8())
        assertEquals("magnet:?xt=urn:btih:dead", body.getString("torrentId"))
        assertTrue(body.getBoolean("prepare"))
    }

    @Test
    fun selectSendsIndexes() {
        transport.enqueue(TestResponse().setBody("""{"ok":true,"selected":[0,2]}"""))
        client.select("abc", setOf(0, 2))
        val req = transport.takeRequest()
        assertEquals("/select", req.path)
        val body = JSONObject(req.body.readUtf8())
        assertEquals("abc", body.getString("id"))
        val selected = mutableSetOf<Int>()
        val selectedArr = body.getJSONArray("selected")
        for (i in 0 until selectedArr.length()) selected.add(selectedArr.getInt(i))
        assertEquals(setOf(0, 2), selected)
    }

    @Test
    fun configureSendsDescriptorsAndSelected() {
        transport.enqueue(TestResponse().setBody("""{"ok":true}"""))
        client.configure("abc", listOf(7, null, 9), setOf(0, 2))
        val req = transport.takeRequest()
        assertEquals("/configure", req.path)
        val body = JSONObject(req.body.readUtf8())
        assertEquals("abc", body.getString("id"))
        val descriptors = body.getJSONArray("descriptors")
        assertEquals(7, descriptors.getInt(0))
        assertTrue(descriptors.isNull(1))
        assertEquals(9, descriptors.getInt(2))
        val selected = mutableSetOf<Int>()
        val selectedArr = body.getJSONArray("selected")
        for (i in 0 until selectedArr.length()) selected.add(selectedArr.getInt(i))
        assertEquals(setOf(0, 2), selected)
    }

    @Test
    fun pauseResumeRemoveHitPaths() {
        transport.enqueue(TestResponse().setBody("""{"ok":true}"""))
        client.pause("abc")
        val pause = transport.takeRequest()
        assertEquals("/pause", pause.path)
        assertEquals("abc", JSONObject(pause.body.readUtf8()).getString("id"))

        transport.enqueue(TestResponse().setBody("""{"ok":true}"""))
        client.resume("abc")
        val resume = transport.takeRequest()
        assertEquals("/resume", resume.path)
        assertEquals("abc", JSONObject(resume.body.readUtf8()).getString("id"))

        transport.enqueue(TestResponse().setBody("""{"ok":true}"""))
        client.remove("abc", destroyStore = false)
        val remove = transport.takeRequest()
        assertEquals("/remove", remove.path)
        val body = JSONObject(remove.body.readUtf8())
        assertEquals("abc", body.getString("id"))
        assertFalse(body.getBoolean("destroyStore"))
    }

    @Test
    fun metadataReturnsTorrentData() {
        transport.enqueue(TestResponse().setBody("""{"torrentData":"d8:announce"}"""))
        assertEquals("d8:announce", client.metadata("abc"))
        assertEquals("/metadata/abc", transport.takeRequest().path)
    }

    @Test
    fun setMaxPeersHitsSettings() {
        transport.enqueue(TestResponse().setBody("""{"maxPeers":24}"""))
        assertEquals(24, client.setMaxPeers(24))
        val req = transport.takeRequest()
        assertEquals("/settings", req.path)
        assertEquals(24, JSONObject(req.body.readUtf8()).getInt("maxPeers"))
    }

    @Test
    fun piecesParsesBucketsAndHitsPath() {
        transport.enqueue(
            TestResponse().setBody(
                """{"id":"abc","infoHash":"dead","generation":3,"timestamp":1700000000000,
                    "totalPieces":4,"pieceLength":16384,"lastPieceLength":1024,"maxBuckets":256,
                    "buckets":[
                      {"start":0,"end":1,"total":2,"selected":2,"verified":1,"receiving":1},
                      {"start":2,"end":3,"total":2,"selected":1,"verified":2,"receiving":0}
                    ]}"""
            )
        )
        val t = client.pieces("abc", 256)
        assertEquals("abc", t.id)
        assertEquals("dead", t.infoHash)
        assertEquals(3L, t.generation)
        assertEquals(1700000000000L, t.timestamp)
        assertEquals(4, t.totalPieces)
        assertEquals(16384L, t.pieceLength)
        assertEquals(1024L, t.lastPieceLength)
        assertEquals(256, t.maxBuckets)
        assertEquals(2, t.buckets.size)
        val first = t.buckets[0]
        assertEquals(0, first.start)
        assertEquals(1, first.end)
        assertEquals(2, first.total)
        assertEquals(2, first.selected)
        assertEquals(1, first.verified)
        assertEquals(1, first.receiving)
        val second = t.buckets[1]
        assertEquals(2, second.start)
        assertEquals(3, second.end)
        assertEquals(2, second.total)
        assertEquals(1, second.selected)
        assertEquals(2, second.verified)
        assertEquals(0, second.receiving)
        assertEquals("/pieces/abc?maxBuckets=256", transport.takeRequest().path)
    }
}

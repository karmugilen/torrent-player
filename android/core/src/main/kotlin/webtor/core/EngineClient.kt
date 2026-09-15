package webtor.core

import org.json.JSONArray
import org.json.JSONObject

data class EngineResponse(
    val statusCode: Int,
    val body: String,
)

fun interface EngineTransport {
    fun request(method: String, path: String, body: String?): EngineResponse
}

data class EngineStats(
    val downloadSpeed: Long,
    val uploadSpeed: Long,
    val progress: Double,
    val ratio: Double,
    val torrents: Int,
    val ctlPort: Int,
    val streamPort: Int,
    val path: String,
)

data class TorrentFile(
    val index: Int,
    val name: String,
    val path: String,
    val length: Long,
    val progress: Double,
    val type: String,
)

data class TorrentStatus(
    val id: String,
    val infoHash: String?,
    val name: String?,
    val magnetURI: String?,
    val ready: Boolean,
    val done: Boolean,
    val paused: Boolean,
    val progress: Double,
    val downloadSpeed: Long,
    val uploadSpeed: Long,
    val numPeers: Int,
    val length: Long,
    val downloaded: Long,
    val uploaded: Long,
    val files: List<TorrentFile>,
    val error: String? = null,
    val timeRemaining: Long? = null,
    val configured: Boolean = false,
    val selected: List<Int> = emptyList(),
)

data class PlayInfo(
    val id: String,
    val fileIndex: Int,
    val name: String,
    val length: Long,
    val streamUrl: String,
)

data class AddResult(val id: String, val infoHash: String?)

data class PieceBucket(
    val start: Int,
    val end: Int,
    val total: Int,
    val selected: Int,
    val verified: Int,
    val receiving: Int,
)

data class PieceTelemetry(
    val id: String,
    val infoHash: String?,
    val generation: Long,
    val timestamp: Long,
    val totalPieces: Int,
    val pieceLength: Long,
    val lastPieceLength: Long,
    val maxBuckets: Int,
    val buckets: List<PieceBucket>,
)

class EngineException(
    message: String,
    val statusCode: Int = 0,
) : RuntimeException(message)

class EngineClient(
    private val transport: EngineTransport,
) {
    fun stats(): EngineStats {
        val json = get("/stats")
        return EngineStats(
            downloadSpeed = json.optLong("downloadSpeed"),
            uploadSpeed = json.optLong("uploadSpeed"),
            progress = json.optDouble("progress"),
            ratio = json.optDouble("ratio"),
            torrents = json.optInt("torrents"),
            ctlPort = json.optInt("ctlPort"),
            streamPort = json.optInt("streamPort"),
            path = json.optString("path"),
        )
    }

    fun add(torrentId: String, prepare: Boolean = false, torrentData: String? = null): AddResult {
        val json = post("/add", JSONObject().put("torrentId", torrentId).put("prepare", prepare).apply {
            if (torrentData != null) put("torrentData", torrentData)
        })
        return AddResult(json.getString("id"), json.nullableString("infoHash"))
    }

    fun torrent(id: String): TorrentStatus {
        val json = get("/torrent/$id")
        return parseTorrent(json)
    }

    fun play(id: String, fileIndex: Int? = null): PlayInfo {
        val body = JSONObject().put("id", id)
        if (fileIndex != null) body.put("fileIndex", fileIndex)
        return parsePlay(post("/play", body))
    }

    fun remove(id: String, destroyStore: Boolean = true) {
        try {
            post("/remove", JSONObject().put("id", id).put("destroyStore", destroyStore))
        } catch (e: EngineException) {
            if (e.statusCode != 404) throw e
        }
    }

    fun pieces(id: String, maxBuckets: Int? = null): PieceTelemetry {
        val query = if (maxBuckets != null) "?maxBuckets=$maxBuckets" else ""
        val json = get("/pieces/$id$query")
        return parsePieces(json)
    }

    fun metadata(id: String): String = get("/metadata/$id").getString("torrentData")

    fun configure(id: String, descriptors: List<Int?>, selected: Set<Int>) {
        post("/configure", JSONObject().put("id", id)
            .put("descriptors", JSONArray().apply { descriptors.forEach { put(it ?: JSONObject.NULL) } })
            .put("selected", JSONArray(selected.toList())))
    }

    fun select(id: String, selected: Set<Int>) {
        post("/select", JSONObject().put("id", id).put("selected", JSONArray(selected.toList())))
    }

    fun pause(id: String) { post("/pause", JSONObject().put("id", id)) }
    fun resume(id: String) { post("/resume", JSONObject().put("id", id)) }

    fun maxPeers(): Int = get("/settings").optInt("maxPeers", 40)

    fun setMaxPeers(maxPeers: Int): Int =
        post("/settings", JSONObject().put("maxPeers", maxPeers)).optInt("maxPeers", maxPeers)

    fun shutdown() {
        post("/shutdown", JSONObject())
    }

    private fun get(path: String): JSONObject = execute("GET", path, null)

    private fun post(path: String, body: JSONObject): JSONObject = execute("POST", path, body.toString())

    private fun execute(method: String, path: String, body: String?): JSONObject {
        val response = transport.request(method, path, body)
        val json = runCatching { JSONObject(response.body) }.getOrNull()
        if (response.statusCode !in 200..299) {
            val err = json?.optString("error").orEmpty().ifEmpty { response.body }
            throw EngineException("Engine ${response.statusCode}: $err", statusCode = response.statusCode)
        }
        return json ?: throw EngineException("The download engine returned an invalid response.")
    }

    companion object {
        private fun parsePlay(json: JSONObject) = PlayInfo(
            id = json.getString("id"),
            fileIndex = json.optInt("fileIndex"),
            name = json.optString("name"),
            length = json.optLong("length"),
            streamUrl = json.getString("streamUrl"),
        )

        fun parsePieces(json: JSONObject): PieceTelemetry {
            val buckets = ArrayList<PieceBucket>()
            val arr: JSONArray = json.optJSONArray("buckets") ?: JSONArray()
            for (i in 0 until arr.length()) {
                val b = arr.getJSONObject(i)
                val start = b.optInt("start")
                val end = b.optInt("end", start)
                buckets.add(
                    PieceBucket(
                        start = start,
                        end = end,
                        total = b.optInt("total", end - start + 1),
                        selected = b.optInt("selected"),
                        verified = b.optInt("verified"),
                        receiving = b.optInt("receiving"),
                    )
                )
            }
            return PieceTelemetry(
                id = json.getString("id"),
                infoHash = json.nullableString("infoHash"),
                generation = json.optLong("generation", 1L),
                timestamp = json.optLong("timestamp"),
                totalPieces = json.optInt("totalPieces"),
                pieceLength = json.optLong("pieceLength"),
                lastPieceLength = json.optLong("lastPieceLength"),
                maxBuckets = json.optInt("maxBuckets", buckets.size),
                buckets = buckets,
            )
        }

        fun parseTorrent(json: JSONObject): TorrentStatus {
            val files = ArrayList<TorrentFile>()
            val arr: JSONArray = json.optJSONArray("files") ?: JSONArray()
            for (i in 0 until arr.length()) {
                val f = arr.getJSONObject(i)
                files.add(
                    TorrentFile(
                        index = f.optInt("index", i),
                        name = f.optString("name"),
                        path = f.optString("path"),
                        length = f.optLong("length"),
                        progress = f.progress(),
                        type = f.optString("type"),
                    )
                )
            }
            val selected = ArrayList<Int>()
            val selectedArr: JSONArray? = json.optJSONArray("selected")
            if (selectedArr != null) {
                for (i in 0 until selectedArr.length()) {
                    selected.add(selectedArr.optInt(i))
                }
            }
            return TorrentStatus(
                id = json.getString("id"),
                infoHash = json.nullableString("infoHash"),
                name = json.nullableString("name"),
                magnetURI = json.nullableString("magnetURI"),
                ready = json.optBoolean("ready"),
                done = json.optBoolean("done"),
                paused = json.optBoolean("paused"),
                progress = json.progress(),
                downloadSpeed = json.optLong("downloadSpeed"),
                uploadSpeed = json.optLong("uploadSpeed"),
                numPeers = json.optInt("numPeers"),
                length = json.optLong("length"),
                downloaded = json.optLong("downloaded"),
                uploaded = json.optLong("uploaded"),
                files = files,
                error = if (json.isNull("error")) null else json.optString("error").ifEmpty { null },
                timeRemaining = json.finiteLong("timeRemaining"),
                configured = json.optBoolean("configured"),
                selected = selected,
            )
        }
    }
}

private fun JSONObject.nullableString(key: String): String? = if (isNull(key)) null else optString(key).ifEmpty { null }

private fun JSONObject.progress(): Double = optDouble("progress", 0.0)
    .let { if (it.isFinite()) it.coerceIn(0.0, 1.0) else 0.0 }

private fun JSONObject.finiteLong(key: String): Long? {
    if (!has(key) || isNull(key)) return null
    val d = optDouble(key)
    if (!d.isFinite()) return null
    return optLong(key)
}

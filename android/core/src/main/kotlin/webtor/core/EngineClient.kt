package webtor.core

import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.TimeUnit

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

class EngineException(message: String) : RuntimeException(message)

class EngineClient(
    private val baseUrl: String,
    private val http: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(5, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .writeTimeout(15, TimeUnit.SECONDS)
        .build(),
) {
    constructor(ctlPort: Int) : this("http://127.0.0.1:$ctlPort")

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
        val json = post("/play", body)
        return PlayInfo(
            id = json.getString("id"),
            fileIndex = json.optInt("fileIndex"),
            name = json.optString("name"),
            length = json.optLong("length"),
            streamUrl = json.getString("streamUrl"),
        )
    }

    fun remove(id: String, destroyStore: Boolean = true) {
        post("/remove", JSONObject().put("id", id).put("destroyStore", destroyStore))
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

    private fun get(path: String): JSONObject = execute(
        Request.Builder().url(baseUrl + path).get().build()
    )

    private fun post(path: String, body: JSONObject): JSONObject = execute(
        Request.Builder()
            .url(baseUrl + path)
            .post(body.toString().toRequestBody(JSON))
            .build()
    )

    private fun execute(request: Request): JSONObject {
        http.newCall(request).execute().use { resp ->
            val text = resp.body?.string().orEmpty()
            val json = runCatching { JSONObject(text) }.getOrNull()
            if (!resp.isSuccessful) {
                val err = json?.optString("error").orEmpty().ifEmpty { text }
                throw EngineException("HTTP ${resp.code}: $err")
            }
            return json ?: JSONObject()
        }
    }

    companion object {
        private val JSON = "application/json; charset=utf-8".toMediaType()

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
                        progress = f.optDouble("progress"),
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
                progress = json.optDouble("progress"),
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

private fun JSONObject.finiteLong(key: String): Long? {
    if (!has(key) || isNull(key)) return null
    val d = optDouble(key)
    if (!d.isFinite()) return null
    return optLong(key)
}

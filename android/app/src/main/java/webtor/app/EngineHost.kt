package webtor.app

import android.content.Context
import android.util.Log
import java.io.File
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import webtor.core.EngineBytesResponse
import webtor.core.EngineResponse
import webtor.core.EngineTransport

/** Private engine state: checkpoints, startup data and engine-owned boundary files. */
fun engineDownloadDir(context: Context): File = File(context.filesDir, "webtorrent-rust")

class EngineHost(private val context: Context) : EngineTransport {
    private val started = AtomicBoolean(false)
    private val ready = CompletableDeferred<Boolean>()
    private val changeVersion = MutableStateFlow(-1L)

    fun downloadDir(): File = engineDownloadDir(context)

    fun start() {
        if (!started.compareAndSet(false, true)) return
        val maxPeers = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
            .getInt("maxPeers", 55)
            .coerceIn(8, 500)
        Thread({
            try {
                check(downloadDir().mkdirs() || downloadDir().isDirectory) { "Could not create engine storage" }
                // Start the engine before the optional ~8MB GeoIP copy so a
                // cold start is not blocked on asset I/O.
                check(startEngine(downloadDir().absolutePath.toByteArray(StandardCharsets.UTF_8), maxPeers) == 0) { "Native engine initialization failed" }
                Log.i(TAG, "native engine ready")
                ready.complete(true)
                // The Go engine before 2.4.0 kept its state here. Nothing reads it now.
                runCatching { File(context.filesDir, "webtorrent").takeIf { it.exists() }?.deleteRecursively() }
                // A memory-mapped country database keeps peer lookup offline.
                // Geo data is optional; a damaged asset must not stop downloads.
                runCatching {
                    val asset = "geoip-country-2026-10.mmdb"
                    val destination = File(downloadDir(), asset)
                    context.assets.openFd(asset).use { descriptor ->
                        if (destination.length() != descriptor.length) {
                            val temporary = File(downloadDir(), "$asset.tmp")
                            context.assets.open(asset).use { input ->
                                temporary.outputStream().use { input.copyTo(it) }
                            }
                            check(temporary.renameTo(destination))
                        }
                    }
                }.onFailure { Log.w(TAG, "Offline peer countries unavailable", it) }
                // One process-owned observer waits in native code. Its StateFlow
                // callbacks are cancellable and never hold a coroutine IO worker.
                var version = -1L
                while (true) {
                    version = waitForEngineEvent(version, 30_000)
                    changeVersion.value = version
                }
            } catch (t: Throwable) {
                ready.complete(false)
                Log.e(TAG, "engine failed", t)
            }
        }, "webtor-engine").start()
    }

    suspend fun awaitReady(timeoutMillis: Long = 20_000): Boolean =
        withTimeoutOrNull(timeoutMillis) { ready.await() } ?: false

    suspend fun awaitChange(afterVersion: Long, timeoutMillis: Long = 30_000): Long =
        withTimeoutOrNull(timeoutMillis) { changeVersion.first { it != afterVersion } } ?: afterVersion

    override fun request(method: String, path: String, body: String?): EngineResponse {
        val response = requestRaw(method, path, body?.toByteArray(StandardCharsets.UTF_8))
        return EngineResponse(response.statusCode, response.body.toString(StandardCharsets.UTF_8))
    }

    override fun requestRaw(method: String, path: String, body: ByteArray?): EngineBytesResponse {
        val payload = requestEngine(method, path, body ?: ByteArray(0))
        val separator = payload.indexOf('\n'.code.toByte())
        if (separator <= 0) {
            return EngineBytesResponse(500, "{\"error\":\"invalid native engine response\"}".toByteArray(StandardCharsets.UTF_8))
        }
        val status = payload.copyOfRange(0, separator).toString(StandardCharsets.UTF_8).toIntOrNull() ?: 500
        return EngineBytesResponse(status, payload.copyOfRange(separator + 1, payload.size))
    }

    private external fun startEngine(downloadDir: ByteArray, maxPeers: Int): Int
    private external fun requestEngine(method: String, path: String, body: ByteArray): ByteArray
    private external fun waitForEngineEvent(afterVersion: Long, timeoutMillis: Long): Long

    external fun openPlayback(id: String, fileIndex: Int): Long
    external fun readPlayback(handle: Long, offset: Long, size: Int, target: ByteArray): Int
    external fun closePlayback(handle: Long)

    external fun openPreview(id: String, fileIndex: Int): Long
    external fun readPreview(handle: Long, offset: Long, size: Int, target: ByteArray): Int
    external fun closePreview(handle: Long)

    companion object {
        const val TAG = "webtor-engine"

        init {
            System.loadLibrary("engine")
        }
    }
}

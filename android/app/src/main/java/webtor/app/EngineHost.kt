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
import webtor.core.EngineResponse
import webtor.core.EngineTransport

class EngineHost(private val context: Context) : EngineTransport {
    private val started = AtomicBoolean(false)
    private val ready = CompletableDeferred<Boolean>()
    private val changeVersion = MutableStateFlow(-1L)

    fun downloadDir(): File = File(context.filesDir, "webtorrent")

    fun start() {
        if (!started.compareAndSet(false, true)) return
        val maxPeers = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
            .getInt("maxPeers", 55)
            .coerceIn(8, 80)
        Thread({
            try {
                check(downloadDir().mkdirs() || downloadDir().isDirectory) { "Could not create engine storage" }
                check(startEngine(downloadDir().absolutePath.toByteArray(StandardCharsets.UTF_8), maxPeers) == 0) { "Native engine initialization failed" }
                Log.i(TAG, "native engine ready")
                ready.complete(true)
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
        val payload = requestEngine(method, path, body.orEmpty().toByteArray(StandardCharsets.UTF_8)).toString(StandardCharsets.UTF_8)
        val separator = payload.indexOf('\n')
        if (separator <= 0) return EngineResponse(500, "{\"error\":\"invalid native engine response\"}")
        val status = payload.substring(0, separator).toIntOrNull() ?: 500
        return EngineResponse(status, payload.substring(separator + 1))
    }

    private external fun startEngine(downloadDir: ByteArray, maxPeers: Int): Int
    private external fun requestEngine(method: String, path: String, body: ByteArray): ByteArray
    private external fun waitForEngineEvent(afterVersion: Long, timeoutMillis: Long): Long

    external fun openPlayback(id: String, fileIndex: Int): Long
    external fun readPlayback(handle: Long, offset: Long, size: Int, target: ByteArray): Int
    external fun closePlayback(handle: Long)

    companion object {
        const val TAG = "webtor-engine"

        init {
            System.loadLibrary("engine")
        }
    }
}

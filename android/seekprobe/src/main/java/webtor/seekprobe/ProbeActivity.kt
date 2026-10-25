package webtor.seekprobe

import android.app.Activity
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.widget.TextView
import java.io.FileInputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import kotlin.random.Random

/**
 * Plays like an external player without decoding: reads the start, then seeks
 * to random points and reads a little from each, as a viewer scrubbing would.
 * Each seek's time from request to its first 64 KiB is logged, then a summary.
 * Extras: seeks (default 30), seed (default 1), watchMs (default 1500).
 */
class ProbeActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val status = TextView(this).apply { text = "Seek probe running" }
        setContentView(status)
        val uri = intent.data ?: return finish()
        val seeks = intent.getIntExtra("seeks", 30)
        val random = Random(intent.getIntExtra("seed", 1))
        val watchMs = intent.getLongExtra("watchMs", 1500L)
        Thread {
            val summary = try {
                probe(uri, seeks, random, watchMs)
            } catch (t: Throwable) {
                Log.e(TAG, "probe failed", t)
                "failed: ${t.message}"
            }
            Log.i(TAG, "SUMMARY $summary")
            runOnUiThread { status.text = summary }
        }.start()
    }

    private fun probe(uri: Uri, seeks: Int, random: Random, watchMs: Long): String {
        val length = open(uri, 0).use { it.length }
        Log.i(TAG, "START uri=$uri length=$length")
        val startup = timeRead(uri, 0, 2 shl 20)
        Log.i(TAG, "STARTUP first=${startup.first}ms 2MiB=${startup.second}ms")
        Thread.sleep(watchMs)
        val times = mutableListOf<Long>()
        repeat(seeks) { i ->
            // Skip the last few MiB so each seek has room to play on.
            val offset = (random.nextLong(length - (8L shl 20))) and (4096L - 1).inv()
            val (first, total) = timeRead(uri, offset, 1 shl 20)
            times += first
            Log.i(TAG, "SEEK $i offset=$offset first=${first}ms 1MiB=${total}ms")
            Thread.sleep(watchMs)
        }
        times.sort()
        return "seeks=${times.size} median=${times[times.size / 2]}ms p90=${times[times.size * 9 / 10]}ms max=${times.last()}ms"
    }

    /** Milliseconds to the first 64 KiB and to [bytes] from [offset]. */
    private fun timeRead(uri: Uri, offset: Long, bytes: Int): Pair<Long, Long> {
        val start = System.nanoTime()
        var first = -1L
        open(uri, offset).use { stream ->
            val buffer = ByteArray(64 shl 10)
            var read = 0
            while (read < bytes) {
                val n = stream.input.read(buffer, 0, minOf(buffer.size, bytes - read))
                if (n < 0) break
                read += n
                if (first < 0 && read >= 64 shl 10) first = (System.nanoTime() - start) / 1_000_000
            }
        }
        return first to (System.nanoTime() - start) / 1_000_000
    }

    private class Opened(val input: InputStream, val length: Long, val close: () -> Unit) : AutoCloseable {
        override fun close() = close.invoke()
    }

    private fun open(uri: Uri, offset: Long): Opened = when (uri.scheme) {
        "http" -> {
            val connection = URL(uri.toString()).openConnection() as HttpURLConnection
            connection.setRequestProperty("Range", "bytes=$offset-")
            connection.connectTimeout = 10_000
            connection.readTimeout = 60_000
            val total = connection.getHeaderField("Content-Range")?.substringAfter('/')?.toLongOrNull()
                ?: connection.contentLengthLong
            Opened(connection.inputStream, total) { connection.disconnect() }
        }
        else -> {
            val descriptor = contentResolver.openFileDescriptor(uri, "r")!!
            val input = FileInputStream(descriptor.fileDescriptor)
            input.channel.position(offset)
            Opened(input, descriptor.statSize) { input.close(); descriptor.close() }
        }
    }

    companion object {
        const val TAG = "SeekProbe"
    }
}

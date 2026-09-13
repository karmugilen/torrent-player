package webtor.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.media.MediaMetadataRetriever
import android.net.Uri
import java.io.File

/** Small, best-effort thumbnail cache for completed local media. */
class ThumbnailRepository(private val context: Context) {
    private val cache = File(context.cacheDir, "thumbnails").apply { mkdirs() }

    fun thumbnail(entry: DownloadEntry): Bitmap {
        if (!entry.complete) return placeholder(entry.title)
        val file = entry.files.firstOrNull { it.index in entry.selected && !it.uri.isNullOrBlank() }
            ?: return placeholder(entry.title)
        val key = (entry.key + file.uri).hashCode().toString()
        val cached = File(cache, key)
        if (cached.exists()) return BitmapFactory.decodeFile(cached.path) ?: placeholder(entry.title)
        val failed = File(cache, "$key.failed")
        if (failed.exists()) return placeholder(entry.title)
        val result: Bitmap? = runCatching {
            val retriever = MediaMetadataRetriever()
            try {
                retriever.setDataSource(context, Uri.parse(file.uri))
                val duration = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0L
                retriever.getFrameAtTime(duration * 35 / 100 * 1000, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
                    ?.let { scale(it) }
            } finally { retriever.release() }
        }.getOrNull()
        if (result == null) failed.writeText("failed") else cached.outputStream().use { result.compress(Bitmap.CompressFormat.JPEG, 82, it) }
        return result ?: placeholder(entry.title)
    }

    private fun placeholder(title: String): Bitmap = Bitmap.createBitmap(320, 180, Bitmap.Config.ARGB_8888).also { bitmap ->
        Canvas(bitmap).apply {
            drawColor(Color.rgb(35, 35, 40))
            val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                color = Color.WHITE
                textSize = 22f
                typeface = Typeface.DEFAULT_BOLD
                textAlign = Paint.Align.CENTER
            }
            drawText(title.take(28), width / 2f, height / 2f, paint)
        }
    }

    private fun scale(bitmap: Bitmap): Bitmap = if (bitmap.width <= 320 && bitmap.height <= 180) bitmap else {
        val ratio = minOf(320f / bitmap.width, 180f / bitmap.height)
        Bitmap.createScaledBitmap(bitmap, (bitmap.width * ratio).toInt().coerceAtLeast(1), (bitmap.height * ratio).toInt().coerceAtLeast(1), true)
    }
}

package webtor.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.widget.Toast

internal fun isLikelyInfoHash(value: String?): Boolean {
    if (value.isNullOrBlank()) return false
    val trimmed = value.trim()
    val isHex40 = trimmed.length == 40 && trimmed.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }
    val isBase32 = trimmed.length == 32 && trimmed.all { it in 'a'..'z' || it in 'A'..'Z' || it in '2'..'7' }
    return isHex40 || isBase32
}

/**
 * Resolves a magnet URI for the given download entry in order:
 * 1. entry.status?.magnetURI
 * 2. entry.source if it looks like a magnet (magnet:?...)
 * 3. build magnet:?xt=urn:btih:<infoHash> from entry.key / status infoHash when possible
 */
fun resolveMagnetUri(entry: DownloadEntry): String? {
    val statusMagnet = entry.status?.magnetURI?.trim()
    if (!statusMagnet.isNullOrEmpty()) {
        return statusMagnet
    }
    val source = entry.source.trim()
    if (source.startsWith("magnet:?", ignoreCase = true)) {
        return source
    }
    val hash = when {
        isLikelyInfoHash(entry.key) -> entry.key.trim()
        !entry.status?.infoHash.isNullOrBlank() -> entry.status?.infoHash?.trim()
        else -> null
    }
    if (hash != null) {
        return "magnet:?xt=urn:btih:$hash"
    }
    return null
}

fun shareMagnetUri(context: Context, uri: String): Boolean {
    val sendIntent = Intent(Intent.ACTION_SEND).apply {
        type = "text/plain"
        putExtra(Intent.EXTRA_TEXT, uri)
    }
    val chooser = Intent.createChooser(sendIntent, "Share magnet link")
    if (context !is Activity) {
        chooser.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    return try {
        context.startActivity(chooser)
        true
    } catch (_: ActivityNotFoundException) {
        Toast.makeText(context, "No app available to share link.", Toast.LENGTH_SHORT).show()
        false
    } catch (_: Exception) {
        false
    }
}

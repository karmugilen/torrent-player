package webtor.app

import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.Base64

/** Largest .torrent accepted from a file or a link. */
internal const val MAX_TORRENT_BYTES = 8 * 1024 * 1024

/** Draft source for a torrent opened from a file; the rest is the file's name. */
internal const val TORRENT_FILE_SOURCE = "torrent-file:"

internal fun torrentFileSource(name: String?): String =
    TORRENT_FILE_SOURCE + (name?.trim()?.takeIf { it.isNotEmpty() } ?: "Torrent file")

internal fun isTorrentFileSource(source: String): Boolean = source.startsWith(TORRENT_FILE_SOURCE)

internal fun torrentFileName(source: String): String = source.removePrefix(TORRENT_FILE_SOURCE)

internal fun isHttpTorrentLink(value: String): Boolean =
    value.startsWith("http://", ignoreCase = true) || value.startsWith("https://", ignoreCase = true)

internal fun isMagnetLink(value: String): Boolean = value.startsWith("magnet:", ignoreCase = true)

/** A .torrent is one bencoded dictionary. The engine does the full parse. */
internal fun looksLikeTorrentFile(bytes: ByteArray): Boolean =
    bytes.size >= 20 && bytes[0] == 'd'.code.toByte() && bytes.any { it == ':'.code.toByte() }

/** Kept for stored library metadata that still arrives as Base64 from `/metadata`. */
internal fun encodeTorrentData(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)

internal fun decodeTorrentData(encoded: String): ByteArray? = runCatching {
    Base64.getDecoder().decode(encoded)
}.getOrNull()

/** Finds the magnet or torrent link in text shared from another app, which may carry a title too. */
internal fun extractTorrentLink(text: String?): String? {
    val value = text?.trim().orEmpty()
    if (value.isEmpty()) return null
    supportedTorrentLink(value)?.let { return it }
    val candidates = Regex("""magnet:\?\S+""", RegexOption.IGNORE_CASE).findAll(value) +
        Regex("""https?://\S+""", RegexOption.IGNORE_CASE).findAll(value)
    return candidates.map { it.value.trimEnd('.', ',', ')', ']', '>', '"', '\'') }
        .firstNotNullOfOrNull { supportedTorrentLink(it) }
}

internal sealed interface FetchedTorrent {
    class File(val bytes: ByteArray) : FetchedTorrent
    /** Some sites answer a torrent link with a redirect to the magnet. */
    class Magnet(val link: String) : FetchedTorrent
}

/** Reads at most [MAX_TORRENT_BYTES]; a longer stream is not a torrent file. */
internal fun readTorrentBytes(input: InputStream): ByteArray {
    val out = java.io.ByteArrayOutputStream()
    val buffer = ByteArray(16 * 1024)
    while (true) {
        val count = input.read(buffer)
        if (count == -1) break
        check(out.size() + count <= MAX_TORRENT_BYTES) { "Torrent files must be smaller than 8 MB." }
        out.write(buffer, 0, count)
    }
    val bytes = out.toByteArray()
    check(bytes.isNotEmpty()) { "This torrent file is empty." }
    return bytes
}

/**
 * The app allows no plain HTTP outside the loopback player link, so a http://
 * torrent link is asked for over HTTPS on the same host.
 */
internal fun secureTorrentUrl(url: URL): URL = when {
    url.protocol.equals("https", true) -> url
    url.protocol.equals("http", true) && (url.port == -1 || url.port == 80) ->
        URL("https", url.host, -1, url.file)
    else -> error("This link uses plain HTTP on a custom port, which Torrent Player does not open. Use an https:// link or download the .torrent file.")
}

/** Blocking: call from a background thread. */
internal fun fetchTorrentLink(link: String, userAgent: String): FetchedTorrent {
    var url = secureTorrentUrl(URL(link))
    repeat(6) {
        val connection = url.openConnection() as HttpURLConnection
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 15_000
            connection.readTimeout = 20_000
            connection.setRequestProperty("User-Agent", userAgent)
            connection.setRequestProperty("Accept", "application/x-bittorrent, application/octet-stream;q=0.9, */*;q=0.5")
            val code = connection.responseCode
            if (code in 300..399) {
                val location = connection.getHeaderField("Location")?.trim().orEmpty()
                check(location.isNotEmpty()) { "The torrent link redirects nowhere." }
                if (isMagnetLink(location)) {
                    return FetchedTorrent.Magnet(
                        supportedTorrentLink(location) ?: error("The torrent link leads to a magnet this app cannot read."),
                    )
                }
                url = secureTorrentUrl(URL(url, location))
                return@repeat
            }
            check(code == HttpURLConnection.HTTP_OK) {
                when (code) {
                    401, 403 -> "The site refused this torrent link (error $code). It may need a login; download the .torrent file in your browser and open it here."
                    404, 410 -> "The torrent link no longer exists (error $code)."
                    else -> "The site could not send the torrent file (error $code)."
                }
            }
            check(connection.contentLengthLong <= MAX_TORRENT_BYTES) { "Torrent files must be smaller than 8 MB." }
            val bytes = connection.inputStream.use(::readTorrentBytes)
            check(looksLikeTorrentFile(bytes)) {
                if (connection.contentType.orEmpty().contains("html", ignoreCase = true))
                    "This link opens a web page, not a torrent file. Copy the magnet link or the .torrent link from that page."
                else "This link did not return a torrent file."
            }
            return FetchedTorrent.File(bytes)
        } finally {
            connection.disconnect()
        }
    }
    error("The torrent link redirects too many times.")
}

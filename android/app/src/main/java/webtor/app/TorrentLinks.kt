package webtor.app

import java.net.URI

/** Validate the whole shared value, including schemes, before passing it to the engine. */
internal fun supportedTorrentLink(value: String?): String? {
    val text = value?.trim()?.takeIf { it.isNotEmpty() && it.length <= 65_536 } ?: return null
    val uri = runCatching { URI(text) }.getOrNull() ?: return null
    return when (uri.scheme?.lowercase()) {
        "magnet" -> text.takeIf {
            val hash = it.substringAfter('?', "").split('&')
                .firstOrNull { part -> part.startsWith("xt=urn:btih:", ignoreCase = true) }
                ?.drop("xt=urn:btih:".length)
                ?.lowercase()
            hash != null && (hash.length == 40 || hash.length == 32)
        }
        "http", "https" -> text.takeIf { !uri.host.isNullOrBlank() && uri.userInfo == null }
        else -> null
    }
}

/** Distinguish a full clipboard insertion from ordinary character-by-character typing. */
internal fun shouldHideKeyboardAfterPaste(previous: String, next: String): Boolean =
    next.length - previous.length >= 20 && looksLikeTorrentSource(next)

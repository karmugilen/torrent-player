package webtor.app

import java.net.URI

/** Validate the whole shared value, including schemes, before passing it to the engine. */
internal fun supportedTorrentLink(value: String?): String? {
    val text = value?.trim()?.takeIf { it.isNotEmpty() && it.length <= 65_536 } ?: return null
    val uri = runCatching { URI(text) }.getOrNull() ?: return null
    return when (uri.scheme?.lowercase()) {
        "magnet" -> text.takeIf { magnetHasSupportedXt(it) }
        "http", "https" -> text.takeIf { !uri.host.isNullOrBlank() && uri.userInfo == null }
        else -> null
    }
}

/** True when the magnet has a usable v1 (`btih`) and/or v2 (`btmh`) exact topic. */
internal fun magnetHasSupportedXt(magnet: String): Boolean {
    val parts = magnet.substringAfter('?', "").split('&')
    for (part in parts) {
        when {
            part.startsWith("xt=urn:btih:", ignoreCase = true) -> {
                val hash = part.drop("xt=urn:btih:".length).lowercase()
                if (hash.length == 40 || hash.length == 32) return true
            }
            part.startsWith("xt=urn:btmh:", ignoreCase = true) -> {
                // BEP 9 multihash: SHA-256 is hex "1220" + 64 hex digits.
                val mh = part.drop("xt=urn:btmh:".length).lowercase()
                if (mh.length == 68 && mh.startsWith("1220") && mh.all { it in '0'..'9' || it in 'a'..'f' }) {
                    return true
                }
            }
        }
    }
    return false
}

/** Distinguish a full clipboard insertion from ordinary character-by-character typing. */
internal fun shouldHideKeyboardAfterPaste(previous: String, next: String): Boolean =
    next.length - previous.length >= 20 && looksLikeTorrentSource(next)

package webtor.app

import android.os.Build

/** Builds a bounded, deliberately non-identifying support report. */
fun buildDiagnosticsReport(entries: List<DownloadEntry>, appVersion: String, now: Long = System.currentTimeMillis()): String {
    val shown = entries.take(50)
    return buildString {
        appendLine("Torrent Player diagnostics")
        appendLine("app=$appVersion")
        appendLine("android=${Build.VERSION.SDK_INT}")
        appendLine("generatedAt=$now")
        appendLine("entries=${entries.size.coerceAtMost(50)}")
        shown.forEachIndexed { i, e ->
            appendLine("entry[$i].stage=${e.stateLabel()}")
            appendLine("entry[$i].files=${e.files.size.coerceAtMost(200)} selected=${e.selected.size.coerceAtMost(200)}")
            appendLine("entry[$i].progress=${(e.progress * 100).toInt().coerceIn(0, 100)}")
            appendLine("entry[$i].errorCategory=${if (e.error == null) "none" else "reported"}")
        }
    }.take(32_000)
}

fun shouldNotifyCompletion(previous: DownloadEntry, current: DownloadEntry): Boolean =
    !previous.complete && current.complete && current.selected.isNotEmpty() && current.completionAck < current.generation

package webtor.app

import webtor.core.EngineClient
import webtor.core.TorrentStatus

fun shouldRestoreAtStartup(entry: DownloadEntry): Boolean =
    !shouldSkipStartupRestore(entry) && entry.engineId == null &&
        (!entry.metadataReady || entry.files.any { it.uri != null })

/** Shared by startup, manual resume and retry; prepare keeps discovery alive without destinations. */
fun EngineClient.addSavedDownload(entry: DownloadEntry) = add(
    entry.source.ifBlank { entry.key },
    prepare = true,
    torrentData = entry.metadata.takeIf { it.isNotBlank() },
)

fun restoredPendingDiscovery(entry: DownloadEntry, id: String, status: TorrentStatus?, startPaused: Boolean) =
    entry.copy(
        engineId = id,
        status = status?.takeIf { it.id == id },
        paused = startPaused,
        error = null,
        lifecycleState = if (startPaused) EntryLifecycleState.PAUSED else EntryLifecycleState.DOWNLOADING,
    )

fun needsDiscoveryStatusRefresh(entry: DownloadEntry): Boolean =
    entry.engineId != null && !entry.paused && !entry.controlsBusy() && !entry.complete &&
        (!entry.metadataReady || entry.liveStatus() == null)

fun parseLibrarySnapshot(
    rawJson: String,
    tombstones: Set<String> = emptySet(),
    tombstoneGens: Map<String, Long> = emptyMap(),
): List<DownloadEntry> {
    val array = try {
        org.json.JSONArray(rawJson)
    } catch (_: Exception) {
        return emptyList()
    }
    val result = ArrayList<DownloadEntry>()
    for (i in 0 until array.length()) {
        try {
            val obj = array.getJSONObject(i)
            val entry = DownloadStorage.parseEntry(obj)
            val tombGen = tombstoneGens[entry.key]
            val isTombstoned = (entry.key in tombstones && tombGen == null) ||
                (tombGen != null && entry.generation <= tombGen)
            if (!isTombstoned) {
                result.add(entry)
            }
        } catch (_: Exception) {
            // Skip individual damaged record to isolate recovery
        }
    }
    return result
}

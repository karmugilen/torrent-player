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

/** Retry the initial read and metadata handoff even when their native event was already consumed. */
fun needsDiscoveryStatusRefresh(entry: DownloadEntry): Boolean =
    entry.engineId != null && !entry.paused && !entry.controlsBusy() && !entry.complete &&
        (!entry.metadataReady || entry.liveStatus() == null)

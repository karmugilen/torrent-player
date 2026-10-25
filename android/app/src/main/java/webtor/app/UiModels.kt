package webtor.app

import webtor.core.TorrentStatus

enum class Screen { Library, Prepare, Settings, Files }

data class PrepareDraft(
    val engineId: String,
    val source: String,
    val torrent: TorrentStatus?,
    val selected: Set<Int>,
    val error: String? = null,
    val busy: Boolean = false,
    val committed: Boolean = false,
    val query: String = "",
)

data class DeleteRequest(
    val key: String,
    val title: String,
)

enum class AppTheme { AERO, ANTIQUITY, RETRO, WADA }

data class UiState(
    val screen: Screen = Screen.Library,
    val openTorrentKey: String? = null,
    val engineReady: Boolean = false,
    val statusLine: String = "Starting download engine…",
    val library: List<DownloadEntry> = emptyList(),
    val addSheetOpen: Boolean = false,
    val magnetDraft: String = "",
    val prepare: PrepareDraft? = null,
    val deleteRequest: DeleteRequest? = null,
    val message: String? = null,
    val error: String? = null,
    val loadWarning: String? = null,
    val freeBytes: Long = 0,
    val managedBytes: Long = 0,
    val cacheBytes: Long = 0,
    val legacyBytes: Long = 0,
    val clearAllConfirm: Boolean = false,
    val restoring: Boolean = false,
    val maxPeers: Int = 55,
    val ramCacheMiB: Int = 64,
    val ramCacheAuto: Boolean = true,
    val seedingEnabled: Boolean = false,
    val seedRatioMilli: Int = 1000,
    val seedTimeMinutes: Int = 60,
    val seedUploadKiB: Int = 256,
    val seedWiFiOnly: Boolean = true,
    val seedChargingOnly: Boolean = false,
    val darkTheme: Boolean = true,
    val appTheme: AppTheme = AppTheme.AERO,
    val dynamicColors: Boolean = true,
    val reducedEffects: Boolean = false,
    val directPlayDefault: Boolean = false,
    val playbackMethod: PlaybackMethod = PlaybackMethod.HTTP,
    val diagnosticsPreview: String? = null,
    val networkPolicy: TransferNetworkPolicy = TransferNetworkPolicy.ANY,
    val networkAllowed: Boolean = true,
    val networkReason: String? = null,
    val antiDpiEnabled: Boolean = false,
    val bypassStatus: webtor.core.BypassStatus? = null,
    val nativeCacheSummary: webtor.core.CacheSummary? = null,
    val storageFolderName: String = "Downloads/Webtor",
    val hasCustomFolder: Boolean = false,
    val storageAccessWarning: String? = null,
    val playBoost: Boolean = true,
    // Built-in playback mode (no longer switches), chosen by the engine's
    // simulated-swarm comparisons: Ghost Pro and optimized seek help; the
    // burst-aware buffer made no measurable difference over adaptive.
    val adaptiveBuffer: Boolean = true,
    val burstAwareBuffer: Boolean = false,
    val peerExploration: Boolean = false,
    val ghostMode: Boolean = false,
    val ghostProMode: Boolean = true,
    val claudeSeekMode: Boolean = true,
    /** Experimental hedged requests for blocks stuck at the playhead. */
    val hedgeRequests: Boolean = false,
    /** Experimental: slow peers leave playback pieces to faster peers. */
    val slowPeerYield: Boolean = false,
    /** Experimental: drop peers that keep us choked. */
    val dropChokers: Boolean = false,
    /** Minutes before an idle Direct Play session is dropped; 0 is off. */
    val idleDropMinutes: Int = 15,
    val seekStats: webtor.core.SeekStats? = null,
    val fileOnlyPlayback: Boolean = true,
    val downloadRateKiB: Int = 0,
    val fastPeerSearch: Boolean = false,
    val pauseOnLowBattery: Boolean = true,
    val batteryPaused: Boolean = false,
    val playNext: Boolean = true,
    val preloadStart: Boolean = false,
    val listenPort: Int = 0,
)

/** User-facing library ordering. Kept pure so it is deterministic across restore/recomposition. */
enum class LibrarySort { RECENT, NAME, PROGRESS, STATUS }

fun LibrarySort.label(): String = when (this) {
    LibrarySort.RECENT -> "Recent"
    LibrarySort.NAME -> "Name"
    LibrarySort.PROGRESS -> "Progress"
    LibrarySort.STATUS -> "Status"
}
enum class TransferNetworkPolicy { ANY, WIFI_ONLY, UNMETERED }
fun transferAllowed(policy: TransferNetworkPolicy, connected: Boolean, wifi: Boolean, metered: Boolean) = connected && when (policy) {
    TransferNetworkPolicy.ANY -> true
    TransferNetworkPolicy.WIFI_ONLY -> wifi
    TransferNetworkPolicy.UNMETERED -> !metered
}

sealed class UiEvent {
    data object PickTorrent : UiEvent()
    data object PickFolder : UiEvent()
    data class PickFileForRepair(val entryKey: String, val fileIndex: Int) : UiEvent()
    data object RequestNotifications : UiEvent()
    data class OpenContent(val uri: String, val mime: String, val name: String,
        val playlist: List<PlaybackItem> = emptyList(), val subtitles: List<PlaybackItem> = emptyList(),
        val positionMs: Long = 0L,
        val bookmarkUris: Map<String, String> = emptyMap(),
        val playbackLeaseKey: String? = null) : UiEvent()
    data object ExitApp : UiEvent()
    data class ShareText(val text: String) : UiEvent()
}

fun UiState.visibleLibrary(sort: LibrarySort = LibrarySort.RECENT): List<DownloadEntry> =
    library.stableLibrarySort(sort)

fun List<DownloadEntry>.stableLibrarySort(sort: LibrarySort): List<DownloadEntry> = when (sort) {
    LibrarySort.RECENT -> sortedWith(compareByDescending<DownloadEntry> { it.addedAt }.thenBy { it.key })
    LibrarySort.NAME -> sortedWith(compareBy<DownloadEntry> { it.title.lowercase() }.thenByDescending { it.addedAt }.thenBy { it.key })
    LibrarySort.PROGRESS -> sortedWith(compareByDescending<DownloadEntry> { it.progress }.thenByDescending { it.addedAt }.thenBy { it.key })
    LibrarySort.STATUS -> sortedWith(compareBy<DownloadEntry> { it.stateLabel() }.thenByDescending { it.addedAt }.thenBy { it.key })
}

fun List<DownloadEntry>.filterLibrary(query: String): List<DownloadEntry> {
    val q = query.trim()
    return if (q.isEmpty()) this else filter { it.title.contains(q, ignoreCase = true) || it.files.any { file -> file.name.contains(q, ignoreCase = true) } }
}

enum class LibraryFilter { ALL, ACTIVE, READY, NEEDS_ATTENTION }

fun LibraryFilter.label(): String = when (this) {
    LibraryFilter.ALL -> "All"
    LibraryFilter.ACTIVE -> "Active"
    LibraryFilter.READY -> "Ready"
    LibraryFilter.NEEDS_ATTENTION -> "Attention"
}

fun LibraryFilter.emptyTitle(): String = when (this) {
    LibraryFilter.ALL -> "No downloads"
    LibraryFilter.ACTIVE -> "No active downloads"
    LibraryFilter.READY -> "No ready downloads"
    LibraryFilter.NEEDS_ATTENTION -> "Nothing needs attention"
}

fun DownloadEntry.matchesFilter(filter: LibraryFilter): Boolean = when (filter) {
    LibraryFilter.ALL -> true
    LibraryFilter.ACTIVE -> !paused && !complete && (lifecycleState == EntryLifecycleState.DOWNLOADING || lifecycleState == EntryLifecycleState.PREPARING || checking)
    LibraryFilter.READY -> complete || (directPlay && files.any { it.isVerifiedComplete || it.verifiedBytes > 0L })
    LibraryFilter.NEEDS_ATTENTION -> error != null || status?.error != null || lifecycleState == EntryLifecycleState.ERROR
}

fun List<DownloadEntry>.applyFilter(filter: LibraryFilter): List<DownloadEntry> =
    if (filter == LibraryFilter.ALL) this else filter { it.matchesFilter(filter) }

data class ContinueWatchingTarget(
    val entry: DownloadEntry,
    val file: SavedFile,
)

private val continueWatchingOrder = compareByDescending<ContinueWatchingTarget> { it.file.bookmarkUpdatedAt }
    .thenByDescending { it.entry.addedAt }
    .thenByDescending { it.entry.key }
    .thenBy { it.file.index }

fun List<DownloadEntry>.continueWatchingTargets(limit: Int = 8): List<ContinueWatchingTarget> {
    if (limit <= 0) return emptyList()
    return flatMap { entry ->
        if (entry.controlsBusy() || !entry.metadataReady) emptyList()
        else entry.files.mapNotNull { file ->
            if (!file.isContinueCandidate(entry)) null else ContinueWatchingTarget(entry, file)
        }
    }.sortedWith(continueWatchingOrder).take(limit)
}

fun List<DownloadEntry>.findContinueWatchingTarget(): ContinueWatchingTarget? =
    continueWatchingTargets(limit = Int.MAX_VALUE).firstOrNull()

/** Best unfinished bookmark inside this torrent, using the continue-watching order. */
fun DownloadEntry.resumeFile(): SavedFile? =
    listOf(this).continueWatchingTargets(limit = 1).firstOrNull()?.file

fun DownloadEntry.hasMultipleVideos(): Boolean =
    files.count { it.name.isVideoName() || it.path.isVideoName() } > 1

fun DownloadEntry.leftOffCaption(): String? {
    val file = resumeFile() ?: return null
    val leftOff = "Left off at ${playbackTime(file.playbackPositionMs)}"
    return if (hasMultipleVideos()) "$leftOff · ${file.name}" else leftOff
}

private fun SavedFile.isContinueCandidate(entry: DownloadEntry): Boolean {
    if (!name.isVideoName() && !path.isVideoName()) return false
    if (playbackPositionMs <= 0L) return false
    if (index !in entry.selected && !isVerifiedComplete) return false
    if (!entry.directPlay && uri.isNullOrBlank()) return false
    if (playbackDurationMs > 0L && playbackPositionMs >= playbackDurationMs - 3000L) return false
    return true
}

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
    val darkTheme: Boolean = true,
    val diagnosticsPreview: String? = null,
    val networkPolicy: TransferNetworkPolicy = TransferNetworkPolicy.ANY,
    val networkAllowed: Boolean = true,
    val networkReason: String? = null,
)

/** User-facing library ordering. Kept pure so it is deterministic across restore/recomposition. */
enum class LibrarySort { RECENT, NAME, PROGRESS, STATUS }
enum class TransferNetworkPolicy { ANY, WIFI_ONLY, UNMETERED }
fun transferAllowed(policy: TransferNetworkPolicy, connected: Boolean, wifi: Boolean, metered: Boolean) = connected && when (policy) {
    TransferNetworkPolicy.ANY -> true
    TransferNetworkPolicy.WIFI_ONLY -> wifi
    TransferNetworkPolicy.UNMETERED -> !metered
}

sealed class UiEvent {
    data object PickTorrent : UiEvent()
    data object RequestNotifications : UiEvent()
    data class OpenContent(val uri: String, val mime: String, val name: String) : UiEvent()
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

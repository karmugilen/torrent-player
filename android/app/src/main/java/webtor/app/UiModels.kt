package webtor.app

import webtor.core.PlayInfo
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
    val statusLine: String = "Starting…",
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
)

sealed class UiEvent {
    data object PickTorrent : UiEvent()
    data object RequestNotifications : UiEvent()
    data class PlayStream(val info: PlayInfo) : UiEvent()
    data class OpenContent(val uri: String, val mime: String, val name: String) : UiEvent()
    data object ExitApp : UiEvent()
}

fun UiState.visibleLibrary(): List<DownloadEntry> = library.sortedByDescending { it.addedAt }

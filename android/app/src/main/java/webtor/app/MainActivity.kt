package webtor.app

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private val vm: MainViewModel by viewModels()
    private val session get() = vm.session

    private var launchedPlaybackUri: String? = null
    private var launchedPlaybackItems: Set<String> = emptySet()
    private var launchedBookmarkUris: Map<String, String> = emptyMap()
    private val viewer = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        @Suppress("DEPRECATION")
        fun number(key: String): Long? = (data?.extras?.get(key) as? Number)?.toLong()
        val position = number("extra_position") ?: number("position")
        val uri = returnedPlaybackUri(data?.data?.toString(), launchedPlaybackUri, launchedPlaybackItems)
        if (position != null && uri != null) session.recordPlaybackPosition(launchedBookmarkUris[uri] ?: uri, position, number("extra_duration") ?: number("duration") ?: 0L)
        (application as WebtorApp).httpPlaybackLease.finish(launchedPlaybackUri)
        session.finishPlayback(launchedBookmarkUris[launchedPlaybackUri] ?: launchedPlaybackUri)
        launchedPlaybackUri = null
        launchedPlaybackItems = emptySet()
        launchedBookmarkUris = emptyMap()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("playbackUri", launchedPlaybackUri)
        outState.putStringArrayList("playbackItems", ArrayList(launchedPlaybackItems))
        outState.putStringArrayList("bookmarkUrisFrom", ArrayList(launchedBookmarkUris.keys))
        outState.putStringArrayList("bookmarkUrisTo", ArrayList(launchedBookmarkUris.values))
        val currentUi = session.ui.value
        if (currentUi.screen == Screen.Settings) {
            outState.putString("savedScreen", Screen.Settings.name)
        } else if (currentUi.screen == Screen.Files && currentUi.openTorrentKey != null) {
            outState.putString("savedScreen", Screen.Files.name)
            outState.putString("savedOpenKey", currentUi.openTorrentKey)
        }
        super.onSaveInstanceState(outState)
    }

    private val torrentPicker = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let(session::openTorrent)
    }
    private val folderPicker = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri -> uri?.let { (application as WebtorApp).session.persistFolder(it) } }
    private var pendingRepairTarget: Pair<String, Int>? = null
    private val fileRepairPicker = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let { pickedUri ->
            pendingRepairTarget?.let { (k, idx) ->
                session.repairFileUri(k, idx, pickedUri)
            }
        }
        pendingRepairTarget = null
    }

    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        launchedPlaybackUri = savedInstanceState?.getString("playbackUri")
        launchedPlaybackItems = savedInstanceState?.getStringArrayList("playbackItems")?.toSet().orEmpty()
        launchedBookmarkUris = savedInstanceState?.getStringArrayList("bookmarkUrisFrom").orEmpty()
            .zip(savedInstanceState?.getStringArrayList("bookmarkUrisTo").orEmpty()).toMap()
        val savedScreen = savedInstanceState?.getString("savedScreen")
        val savedOpenKey = savedInstanceState?.getString("savedOpenKey")
        if (savedScreen == Screen.Settings.name) {
            session.openSettings()
        }
        lifecycleScope.launch {
            session.awaitInitialization()
            if (savedScreen == Screen.Files.name && savedOpenKey != null) {
                if (session.ui.value.library.any { it.key == savedOpenKey }) {
                    session.openDetails(savedOpenKey)
                }
            }
            intent.getStringExtra("open_key")?.let { key ->
                if (session.ui.value.library.any { it.key == key }) {
                    session.openDetails(key)
                }
            }
        }
        if (savedInstanceState == null) handleIncoming(intent)
        setContent {
            val state by session.ui.collectAsState()
            val screenStates = rememberSaveableStateHolder()
            GaleTheme(
                dark = state.darkTheme,
                theme = state.appTheme,
                reducedEffects = state.reducedEffects,
                dynamicColors = state.dynamicColors,
            ) {
                ThemedScreenContainer {
                    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                        if (!state.addSheetOpen) screenStates.SaveableStateProvider(if (state.screen == Screen.Files) "files:${state.openTorrentKey}" else state.screen.name) {
                        when (state.screen) {
                            Screen.Prepare -> {
                                val draft = state.prepare
                                if (draft != null) {
                                    PrepareScreen(
                                        draft = draft,
                                        freeBytes = state.freeBytes,
                                        onToggle = session::toggleFile,
                                        onSelectAll = session::selectAllFiles,
                                        onSelectNone = session::selectNoFiles,
                                        onStart = session::startDownload,
                                        onDirectPlay = session::startDirectPlay,
                                        directPlayDefault = state.directPlayDefault,
                                        onCancel = session::cancelPrepare,
                                    )
                                } else {
                                    LibraryLayer(state)
                                }
                            }
                            Screen.Settings -> SettingsScreen(
                                state = state,
                                onBack = session::closeSettings,
                                onMaxPeers = session::setMaxPeers,
                                onMaxPeersCommit = session::commitMaxPeers,
                                onRamCache = session::setRamCacheMiB,
                                onAutoRamCache = session::setAutoRamCache,
                                onSeeding = session::setSeedingEnabled,
                                onSeedRatio = session::setSeedRatio,
                                onSeedTime = session::setSeedTime,
                                onSeedUpload = session::setSeedUpload,
                                onSeedWiFi = session::setSeedWiFi,
                                onSeedCharging = session::setSeedCharging,
                                onDarkTheme = session::setDarkTheme,
                                onAppTheme = session::setAppTheme,
                                onDynamicColors = session::setDynamicColors,
                                onReducedEffects = session::setReducedEffects,
                                onDirectPlayDefault = session::setDirectPlayDefault,
                                onPlaybackMethod = session::setPlaybackMethod,
                                onAntiDpi = session::setAntiDpi,
                                onCleanup = session::cleanupCache,
                                onClearAll = session::requestClearAll,
                                onStopAll = session::stopAll,
                                onStopAllAndExit = { session.stopAllAndExit() },
                                onClearNativeCategory = { session.clearNativeCache(listOf(it)) },
                                onPickFolder = session::pickFolder,
                                onClearFolder = session::clearFolder,
                                onPlayBoost = session::setPlayBoost,
                                onAdaptiveBuffer = session::setAdaptiveBuffer,
                                onBurstAwareBuffer = session::setBurstAwareBuffer,
                                onPeerExploration = session::setPeerExploration,
                                onGhostMode = session::setGhostMode,
                                onFileOnlyPlayback = session::setFileOnlyPlayback,
                                onNetworkPolicy = session::setNetworkPolicy,
                                onDownloadRateKiB = session::setDownloadRateKiB,
                                onFastPeerSearch = session::setFastPeerSearch,
                                onPauseOnLowBattery = session::setPauseOnLowBattery,
                                onPlayNext = session::setPlayNext,
                                onPreloadStart = session::setPreloadStart,
                            )
                            Screen.Files -> {
                                val entry = state.library.find { it.key == state.openTorrentKey }
                                if (entry == null) LibraryLayer(state)
                                else TorrentFilesScreen(
                                    entry = entry,
                                    error = state.error,
                                    onBack = session::closeFiles,
                                    onPlay = { index -> session.play(entry, index) },
                                    onToggleFile = { index -> session.toggleDownloadFile(entry.key, index) },
                                    onPause = { session.pause(entry) },
                                    onResume = { session.resume(entry) },
                                    onDelete = { session.requestDelete(entry) },
                                )
                            }
                            Screen.Library -> LibraryLayer(state)
                        }
                        }
                    if (state.addSheetOpen) {
                        val prefetch = state.prepare?.takeIf {
                            it.source == state.magnetDraft.trim() ||
                                (state.magnetDraft.isBlank() && it.source.startsWith("${cacheDir.absolutePath}/import-"))
                        }
                        val existing = existingLibraryEntry(state)
                        AddSheet(
                            magnet = state.magnetDraft,
                            engineReady = state.engineReady,
                            error = state.error,
                            status = prefetchStatus(prefetch),
                            connecting = prefetch != null && prefetch.torrent?.ready != true && existing == null,
                            onMagnet = session::setMagnet,
                            onAdd = session::addCurrent,
                            onOpenFile = {
                                pickTorrentFile()
                            },
                            onDismiss = session::closeAddSheet,
                            draft = prefetch,
                            freeBytes = state.freeBytes,
                            destination = state.storageFolderName,
                            onToggle = session::toggleFile,
                            onSelectAll = session::selectAllFiles,
                            onSelectNone = session::selectNoFiles,
                            onSelectVideos = session::selectVideoFiles,
                            onDownload = session::startDownload,
                            onDirectPlay = session::startDirectPlay,
                            directPlayDefault = state.directPlayDefault,
                            existingEntry = existing,
                            onOpenExisting = existing?.let {
                                {
                                    session.closeAddSheet()
                                    session.showFiles(it)
                                }
                            },
                        )
                    }
                    state.deleteRequest?.let { req ->
                        DeleteDialog(
                            title = req.title,
                            onKeepFiles = session::deleteKeep,
                            onEraseFiles = session::deleteErase,
                            onDismiss = session::dismissDelete,
                        )
                    }
                    if (state.clearAllConfirm) {
                        ClearAllDialog(
                            onConfirm = session::confirmClearAll,
                            onDismiss = session::dismissClearAll,
                        )
                    }
                    state.diagnosticsPreview?.let { report ->
                        AlertDialog(onDismissRequest = session::closeDiagnostics, title = { androidx.compose.material3.Text("Diagnostics preview") }, text = { androidx.compose.material3.Text(report) }, confirmButton = { androidx.compose.material3.TextButton(onClick = session::shareDiagnostics) { androidx.compose.material3.Text("Share") } }, dismissButton = { androidx.compose.material3.TextButton(onClick = session::closeDiagnostics) { androidx.compose.material3.Text("Close") } })
                    }
                }
                }
            }
            LaunchedEffect(Unit) {
                lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                    session.events.collect { event -> handleEvent(event) }
                }
            }
        }
    }

    @androidx.compose.runtime.Composable
    private fun LibraryLayer(state: UiState) {
        LibraryScreen(
            state = state,
            onAdd = session::openAddSheet,
            onSettings = session::openSettings,
            onPause = session::pause,
            onResume = session::resume,
            onPlay = { entry, index -> session.play(entry, index) },
            onOpen = session::showFiles,
            onDelete = session::requestDelete,
            onRetry = session::retry,
            onMessageShown = session::consumeMessage,
            onPauseBatch = session::pauseBatch,
            onResumeBatch = session::resumeBatch,
            onDeleteBatch = session::deleteBatch,
        )
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncoming(intent)
    }

    override fun onStart() {
        super.onStart()
        (application as WebtorApp).setCurrentActivity(this)
        session.onForeground()
    }

    override fun onStop() {
        if (isFinishing) (application as WebtorApp).clearCurrentActivity(this)
        super.onStop()
    }

    override fun onDestroy() {
        (application as WebtorApp).clearCurrentActivity(this)
        super.onDestroy()
    }

    private fun handleEvent(event: UiEvent) {
        when (event) {
            UiEvent.PickTorrent -> pickTorrentFile()
            UiEvent.PickFolder -> folderPicker.launch(null)
            is UiEvent.PickFileForRepair -> {
                pendingRepairTarget = Pair(event.entryKey, event.fileIndex)
                fileRepairPicker.launch(arrayOf("*/*"))
            }
            UiEvent.RequestNotifications -> requestNotifications()
            is UiEvent.OpenContent -> openContent(event)
            is UiEvent.ShareText -> startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).apply { type = "text/plain"; putExtra(Intent.EXTRA_TEXT, event.text) }, "Share diagnostics"))
            UiEvent.ExitApp -> finish()
        }
    }

    private fun handleIncoming(intent: Intent?) {
        if (intent?.action !in setOf(Intent.ACTION_VIEW, Intent.ACTION_SEND)) return
        try {
            val stream = if (Build.VERSION.SDK_INT >= 33) {
                intent?.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent?.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
            }
            val source = intent?.data ?: stream
            if (source?.scheme == "content") {
                session.openTorrent(source)
                return
            }
            // Never let an exported activity import another app's chosen private file path.
            val text = source?.toString() ?: intent?.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
            val supported = supportedTorrentLink(text)
            if (supported != null) {
                val hash = infoHashFromMagnet(supported)
                val existing = hash?.let { h -> session.ui.value.library.find { it.key.equals(h, true) } }
                if (existing != null) {
                    session.openDetails(existing.key)
                } else {
                    session.add(supported)
                }
            } else {
                session.showError("Open a magnet link, an HTTP(S) torrent URL, or share a .torrent file.")
            }
        } catch (_: RuntimeException) {
            session.showError("Cannot read this shared item. Try opening it with the torrent file picker.")
        }
    }

    private fun pickTorrentFile() {
        try {
            torrentPicker.launch(arrayOf("application/x-bittorrent", "*/*"))
        } catch (_: ActivityNotFoundException) {
            session.showError("No file picker is available on this device.")
        } catch (_: SecurityException) {
            session.showError("The file picker could not be opened. Try sharing the torrent from your Files app.")
        }
    }

    private fun requestNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    private fun openContent(event: UiEvent.OpenContent) {
        val uri = Uri.parse(event.uri)
        val view = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, event.mime)
            val grants = (listOf(PlaybackItem(event.uri, event.name)) + event.playlist + event.subtitles)
                .distinctBy { it.uri }.filter { Uri.parse(it.uri).scheme == "content" }
            if (grants.isNotEmpty()) {
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                clipData = ClipData.newRawUri(grants.first().name, Uri.parse(grants.first().uri)).apply {
                    grants.drop(1).forEach { addItem(ClipData.Item(Uri.parse(it.uri))) }
                }
            }
            putExtra(Intent.EXTRA_TITLE, event.name)
            putExtra("title", event.name)
            putExtra("return_result", true)
            // MX-compatible players use an integer; VLC accepts either numeric type.
            putExtra("position", event.positionMs.coerceAtMost(Int.MAX_VALUE.toLong()).toInt())
            putExtra("video_list", event.playlist.map { Uri.parse(it.uri) }.toTypedArray())
            putExtra("video_list.name", event.playlist.map { it.name }.toTypedArray())
            putExtra("video_list.filename", event.playlist.map { it.name }.toTypedArray())
            putExtra("video_list_is_explicit", true)
            putExtra("subs", event.subtitles.map { Uri.parse(it.uri) }.toTypedArray())
            putExtra("subs.name", event.subtitles.map { it.name }.toTypedArray())
            putExtra("subs.enable", event.subtitles.take(1).map { Uri.parse(it.uri) }.toTypedArray())
            event.subtitles.firstOrNull()?.let { putExtra("subtitles_location", it.uri) }
        }
        launchedPlaybackUri = event.uri
        launchedPlaybackItems = event.playlist.map { it.uri }.toSet()
        launchedBookmarkUris = event.bookmarkUris
        event.playbackLeaseKey?.let { (application as WebtorApp).httpPlaybackLease.begin(event.uri, it) }
        if (!launchViewer(view)) {
            (application as WebtorApp).httpPlaybackLease.finish(event.uri)
            session.finishPlayback(event.bookmarkUris[event.uri] ?: event.uri)
            launchedPlaybackUri = null
            launchedPlaybackItems = emptySet()
            launchedBookmarkUris = emptyMap()
        }
    }

    private fun launchViewer(intent: Intent): Boolean {
        val isMedia = intent.type?.let { it.startsWith("video/") || it.startsWith("audio/") } == true
        val missing = if (isMedia) "Install VLC, mpv, or another media player."
            else "No installed app can open this file type. The file is saved in Downloads/Webtor."
        try {
            if (intent.resolveActivity(packageManager) == null) {
                session.showError(missing)
                return false
            }
            // Launch the implicit intent directly so Android owns the choice and
            // can offer its native Just once / Always player selection.
            viewer.launch(intent)
            return true
        } catch (_: ActivityNotFoundException) {
            session.showError(missing)
        } catch (_: SecurityException) {
            session.showError("The selected app could not access this file. Choose another app when Android asks.")
        }
        return false
    }
}

private fun existingLibraryEntry(state: UiState): DownloadEntry? {
    val hash = infoHashFromMagnet(state.magnetDraft) ?: state.prepare?.torrent?.infoHash
    return state.library.find { entry ->
        (hash != null && entry.key.equals(hash, true)) ||
            (state.prepare?.engineId != null && entry.engineId == state.prepare.engineId)
    }
}

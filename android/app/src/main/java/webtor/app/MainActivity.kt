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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import webtor.core.PlayInfo

class MainActivity : ComponentActivity() {
    private val vm: MainViewModel by viewModels()
    private val session get() = vm.session

    private val torrentPicker = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let(session::openTorrent)
    }

    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) handleIncoming(intent)
        setContent {
            val state by session.ui.collectAsState()
            GaleTheme(dark = state.darkTheme) {
                Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
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
                            onDarkTheme = session::setDarkTheme,
                            onCleanup = session::cleanupCache,
                            onClearAll = session::requestClearAll,
                            onStopAll = session::stopAll,
                            onStopAllAndExit = { session.stopAllAndExit() },
                        )
                        Screen.Files -> {
                            val entry = state.library.find { it.key == state.openTorrentKey }
                            if (entry == null) LibraryLayer(state)
                            else TorrentFilesScreen(
                                entry = entry,
                                error = state.error,
                                onBack = session::closeFiles,
                                onPlay = { index -> session.play(entry, index) },
                                onPause = { session.pause(entry) },
                                onResume = { session.resume(entry) },
                                onDelete = { session.requestDelete(entry) },
                            )
                        }
                        Screen.Library -> LibraryLayer(state)
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
                            onToggle = session::toggleFile,
                            onSelectAll = session::selectAllFiles,
                            onSelectNone = session::selectNoFiles,
                            onDownload = session::startDownload,
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
            onPlay = { session.play(it) },
            onOpen = session::showFiles,
            onDelete = session::requestDelete,
            onRetry = session::retry,
            onMessageShown = session::consumeMessage,
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
            UiEvent.RequestNotifications -> requestNotifications()
            is UiEvent.PlayStream -> openPlayer(event.info)
            is UiEvent.OpenContent -> openContent(event.uri, event.mime, event.name)
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

    private fun openPlayer(info: PlayInfo): Boolean {
        val view = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(Uri.parse(info.streamUrl), "video/*")
            putExtra(Intent.EXTRA_TITLE, info.name)
            putExtra("title", info.name)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        return launchViewer(view)
    }

    private fun openContent(uriString: String, mime: String, name: String) {
        val uri = Uri.parse(uriString)
        val view = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, mime)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            clipData = ClipData.newRawUri(name, uri)
        }
        launchViewer(view)
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
            startActivity(intent)
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

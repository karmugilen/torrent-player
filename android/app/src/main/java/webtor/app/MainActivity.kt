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
import kotlinx.coroutines.flow.collectLatest
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
                            onPlayer = session::setPlayer,
                            onDarkTheme = session::setDarkTheme,
                            onCleanup = session::cleanupCache,
                            onClearAll = session::requestClearAll,
                        )
                        Screen.Files -> {
                            val entry = state.library.find { it.key == state.openTorrentKey }
                            if (entry == null) LibraryLayer(state)
                            else TorrentFilesScreen(
                                entry = entry,
                                error = state.error,
                                onBack = session::closeFiles,
                                onPlay = { index -> session.play(entry, index) },
                            )
                        }
                        Screen.Library -> LibraryLayer(state)
                    }
                    if (state.addSheetOpen) {
                        val prefetch = state.prepare?.takeIf { it.source == state.magnetDraft.trim() }
                        AddSheet(
                            magnet = state.magnetDraft,
                            engineReady = state.engineReady,
                            error = state.error,
                            status = prefetchStatus(prefetch),
                            connecting = prefetch != null && prefetch.torrent?.ready != true,
                            onMagnet = session::setMagnet,
                            onAdd = session::addCurrent,
                            onOpenFile = {
                                torrentPicker.launch(arrayOf("application/x-bittorrent", "*/*"))
                            },
                            onDismiss = session::closeAddSheet,
                            draft = prefetch,
                            freeBytes = state.freeBytes,
                            onToggle = session::toggleFile,
                            onSelectAll = session::selectAllFiles,
                            onSelectNone = session::selectNoFiles,
                            onDownload = session::startDownload,
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
                session.events.collectLatest { event -> handleEvent(event) }
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
            onPlay = session::open,
            onOpen = session::showFiles,
            onDelete = session::requestDelete,
            onRetry = session::retry,
            onMessageShown = session::consumeMessage,
        )
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIncoming(intent)
    }

    private fun handleEvent(event: UiEvent) {
        when (event) {
            UiEvent.PickTorrent -> torrentPicker.launch(arrayOf("application/x-bittorrent", "*/*"))
            UiEvent.RequestNotifications -> requestNotifications()
            is UiEvent.PlayStream -> openPlayer(event.info)
            is UiEvent.OpenContent -> openContent(event.uri, event.mime, event.name)
            UiEvent.ExitApp -> finish()
        }
    }

    private fun handleIncoming(intent: Intent?) {
        if (intent == null) return
        val stream = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }
        val fileUri = intent.data?.takeIf { it.scheme == "content" || it.scheme == "file" } ?: stream
        if (fileUri != null && intent.action != Intent.ACTION_MAIN) {
            val asText = fileUri.toString()
            if (asText.startsWith("magnet:")) session.add(asText) else session.openTorrent(fileUri)
            return
        }
        val uri = intent.data?.toString() ?: intent.getStringExtra(Intent.EXTRA_TEXT)
        if (uri != null && (uri.startsWith("magnet:") || uri.endsWith(".torrent") || uri.startsWith("http"))) {
            session.add(uri.trim())
        }
    }

    private fun requestNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    private fun openPlayer(info: PlayInfo) {
        val view = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(Uri.parse(info.streamUrl), "video/*")
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        launchViewer(view, info.name)
    }

    private fun openContent(uriString: String, mime: String, name: String) {
        val uri = Uri.parse(uriString)
        val view = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, mime)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            clipData = ClipData.newRawUri(name, uri)
        }
        launchViewer(view, name)
    }

    private fun launchViewer(intent: Intent, name: String) {
        val missing = "Install VLC, mpv, or another video player."
        val chosen = session.preferredPlayer()
        try {
            if (chosen != null) {
                val targeted = Intent(intent).setClassName(chosen.packageName, chosen.activity)
                if (targeted.resolveActivity(packageManager) != null) {
                    startActivity(targeted)
                    return
                }
            }
            if (intent.resolveActivity(packageManager) == null) {
                session.showError(missing)
                return
            }
            if (chosen == null) {
                startActivity(Intent.createChooser(intent, "Play $name"))
            } else {
                startActivity(intent)
            }
        } catch (_: ActivityNotFoundException) {
            session.showError(missing)
        }
    }
}

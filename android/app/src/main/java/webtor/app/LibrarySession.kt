package webtor.app

import android.app.Application
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import webtor.core.EngineClient
import webtor.core.EngineException
import webtor.core.TorrentStatus

class LibrarySession(private val app: Application) {
    private val client = EngineClient(NodeHost.DEFAULT_CTL_PORT)
    private val storage = DownloadStorage(app)
    private val settings = app.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val io = Dispatchers.IO
    private val mutex = Mutex()
    private val started = AtomicBoolean(false)
    private var lastPersistAt = 0L
    private var debounceJob: Job? = null
    private var prefetchJob: Job? = null
    private var storageStatsJob: Job? = null
    private var lastServiceUpdate = 0L

    private val _ui = MutableStateFlow(UiState())
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    private val _events = MutableSharedFlow<UiEvent>(extraBufferCapacity = 24)
    val events: SharedFlow<UiEvent> = _events.asSharedFlow()

    fun start() {
        if (!started.compareAndSet(false, true)) return
        // Reset previous custom connection tuning once; subsequent user changes persist.
        if (!settings.getBoolean("peerDefaultsV2", false)) {
            settings.edit().putInt("maxPeers", 55).putBoolean("peerDefaultsV2", true).apply()
        }
        refreshStorageStats()
        scope.launch {
            val loaded = withContext(io) { storage.load() }.map { it.copy(engineId = null) }
            val players = queryVideoPlayers(app.packageManager, app.packageName)
            val savedPlayer = settings.getString("playerPackage", "") ?: ""
            val savedActivity = settings.getString("playerActivity", "") ?: ""
            val player = findPlayer(players, savedPlayer)
            _ui.update {
                it.copy(
                    library = loaded,
                    loadWarning = storage.loadWarning,
                    maxPeers = settings.getInt("maxPeers", 55).coerceIn(8, 80),
                    restoring = loaded.isNotEmpty(),
                    statusLine = if (loaded.isEmpty()) it.statusLine else "Restoring downloads…",
                    players = players,
                    playerPackage = player?.packageName ?: "",
                    playerActivity = player?.activity ?: "",
                    darkTheme = settings.getBoolean("darkTheme", true),
                )
            }
            if (savedPlayer.isNotEmpty() && player == null) {
                settings.edit().remove("playerPackage").remove("playerActivity").apply()
            }
            refreshStorageStats()
            waitForEngine()
            if (_ui.value.engineReady) {
                runCatching { withContext(io) { client.setMaxPeers(_ui.value.maxPeers) } }
                restoreAll(loaded)
            }
            _ui.update { it.copy(restoring = false, statusLine = if (it.engineReady) "Ready" else it.statusLine) }
            persist()
            syncService()
        }
        scope.launch { pollLoop() }
    }

    fun setMagnet(value: String) {
        _ui.update { it.copy(magnetDraft = value, error = null) }
        debounceJob?.cancel()
        debounceJob = scope.launch {
            delay(180)
            val src = _ui.value.magnetDraft.trim()
            if (!looksLikeTorrentSource(src)) {
                dropUncommittedPrefetch(exceptSource = src)
                return@launch
            }
            if (_ui.value.prepare?.source == src) return@launch
            prefetchJob?.cancel()
            prefetchJob = scope.launch { addTorrent(src, commit = false) }
        }
    }
    fun openAddSheet() = _ui.update { it.copy(addSheetOpen = true, error = null) }
    fun closeAddSheet() {
        debounceJob?.cancel()
        _ui.update { it.copy(addSheetOpen = false, error = null) }
        dropUncommittedPrefetch()
    }
    fun openSettings() {
        refreshStorageStats()
        refreshPlayers()
        _ui.update { it.copy(screen = Screen.Settings, error = null) }
    }

    fun setDarkTheme(dark: Boolean) {
        settings.edit().putBoolean("darkTheme", dark).apply()
        _ui.update { it.copy(darkTheme = dark) }
    }

    fun setPlayer(app: PlayerApp?) {
        settings.edit()
            .putString("playerPackage", app?.packageName ?: "")
            .putString("playerActivity", app?.activity ?: "")
            .apply()
        _ui.update {
            it.copy(
                playerPackage = app?.packageName ?: "",
                playerActivity = app?.activity ?: "",
            )
        }
    }

    fun preferredPlayer(): PlayerApp? = findPlayer(_ui.value.players, _ui.value.playerPackage)

    private fun refreshPlayers() {
        val players = queryVideoPlayers(app.packageManager, app.packageName)
        val current = findPlayer(players, _ui.value.playerPackage)
        _ui.update {
            it.copy(
                players = players,
                playerPackage = current?.packageName ?: "",
                playerActivity = current?.activity ?: it.playerActivity,
            )
        }
        if (_ui.value.playerPackage.isEmpty() && (settings.getString("playerPackage", "") ?: "").isNotEmpty()) {
            settings.edit().remove("playerPackage").remove("playerActivity").apply()
        }
    }
    fun closeSettings() = _ui.update { it.copy(screen = Screen.Library, clearAllConfirm = false) }
    fun setMaxPeers(value: Int) {
        val n = value.coerceIn(8, 80)
        settings.edit().putInt("maxPeers", n).apply()
        _ui.update { it.copy(maxPeers = n) }
    }

    fun commitMaxPeers() {
        val n = _ui.value.maxPeers
        scope.launch {
            if (_ui.value.engineReady) runCatching { withContext(io) { client.setMaxPeers(n) } }
        }
    }
    fun consumeMessage() = _ui.update { it.copy(message = null) }
    fun consumeError() = _ui.update { it.copy(error = null) }
    fun showError(message: String) {
        _ui.update { state ->
            if (state.screen == Screen.Prepare && state.prepare != null) {
                state.copy(prepare = state.prepare.copy(error = message))
            } else {
                state.copy(error = message)
            }
        }
    }
    fun requestDelete(entry: DownloadEntry) = _ui.update { it.copy(deleteRequest = DeleteRequest(entry.key, entry.title)) }
    fun dismissDelete() = _ui.update { it.copy(deleteRequest = null) }
    fun requestClearAll() = _ui.update { it.copy(clearAllConfirm = true) }
    fun dismissClearAll() = _ui.update { it.copy(clearAllConfirm = false) }
    fun pickTorrent() { _events.tryEmit(UiEvent.PickTorrent) }

    fun addCurrent() {
        debounceJob?.cancel()
        val id = _ui.value.magnetDraft.trim()
        if (id.isEmpty()) {
            _ui.update { it.copy(error = "Paste a magnet or torrent URL.") }
            return
        }
        val prep = _ui.value.prepare
        if (prep != null && prep.source == id) {
            _ui.update {
                it.copy(
                    prepare = prep.copy(committed = true, error = null),
                    screen = Screen.Prepare,
                    addSheetOpen = false,
                    error = null,
                    magnetDraft = "",
                )
            }
            if (prep.torrent?.ready != true && prefetchJob?.isActive != true) {
                prefetchJob = scope.launch { runCatching { waitReady(prep.engineId) } }
            }
            return
        }
        add(id)
    }

    fun add(torrentId: String) {
        val id = torrentId.trim()
        if (id.isEmpty()) {
            _ui.update { it.copy(error = "Paste a magnet or torrent URL.") }
            return
        }
        debounceJob?.cancel()
        prefetchJob?.cancel()
        prefetchJob = scope.launch { addTorrent(id, commit = true) }
    }

    fun openTorrent(uri: Uri) {
        scope.launch {
            try {
                val file = withContext(io) {
                    val dest = File.createTempFile("import-", ".torrent", app.cacheDir)
                    try {
                        app.contentResolver.openInputStream(uri)?.use { input ->
                            dest.outputStream().use { input.copyTo(it) }
                        } ?: error("Cannot open torrent file")
                        dest
                    } catch (t: Throwable) {
                        dest.delete()
                        throw t
                    }
                }
                try {
                    addTorrent(file.absolutePath)
                } finally {
                    file.delete()
                }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t), addSheetOpen = false) }
            }
        }
    }

    fun cancelPrepare() {
        debounceJob?.cancel()
        prefetchJob?.cancel()
        scope.launch {
            mutex.withLock {
                val id = _ui.value.prepare?.engineId
                _ui.update { it.copy(prepare = null, screen = Screen.Library, error = null) }
                if (id != null && _ui.value.library.none { it.engineId == id }) {
                    runCatching { withContext(io) { client.remove(id, true) } }
                }
            }
        }
    }

    fun toggleFile(index: Int) {
        _ui.update { state ->
            val draft = state.prepare ?: return@update state
            val selected = draft.selected.toMutableSet()
            if (!selected.add(index)) selected.remove(index)
            state.copy(prepare = draft.copy(selected = selected, error = null))
        }
        syncPrepareSelection()
    }

    fun selectAllFiles() {
        _ui.update { state ->
            val files = state.prepare?.torrent?.files ?: return@update state
            state.copy(prepare = state.prepare.copy(selected = files.map { it.index }.toSet(), error = null))
        }
        syncPrepareSelection()
    }

    fun selectNoFiles() {
        _ui.update { state ->
            val draft = state.prepare ?: return@update state
            state.copy(prepare = draft.copy(selected = emptySet(), error = null))
        }
        syncPrepareSelection()
    }

    fun startDownload() {
        scope.launch {
            mutex.withLock {
                val draft = _ui.value.prepare ?: return@withLock
                val torrent = draft.torrent
                if (torrent?.ready != true) {
                    setPrepareError("Still fetching torrent info.")
                    return@withLock
                }
                if (draft.selected.isEmpty()) {
                    setPrepareError("Select at least one file to download.")
                    return@withLock
                }
                if (Build.VERSION.SDK_INT < 29) {
                    setPrepareError("Torrent Player needs Android 10 or newer to save into Downloads.")
                    return@withLock
                }
                val needed = torrent.files.filter { it.index in draft.selected }.sumOf { it.length }
                val free = storage.freeBytes
                if (needed > free) {
                    setPrepareError(
                        "Not enough storage. This download needs ${formatBytes(needed)}; ${formatBytes(free)} is available.",
                    )
                    return@withLock
                }
                _ui.update { it.copy(prepare = draft.copy(busy = true, error = null)) }
                _events.tryEmit(UiEvent.RequestNotifications)
                try {
                    val metadata = withContext(io) { client.metadata(draft.engineId) }
                    val key = torrent.infoHash ?: draft.engineId
                    val skeleton = DownloadEntry(
                        key = key,
                        title = torrent.name ?: key,
                        source = torrent.magnetURI ?: draft.source,
                        metadata = metadata,
                        engineId = draft.engineId,
                        destination = "Downloads/Webtor",
                        files = torrent.files.map { SavedFile(it.index, it.name, it.path, it.length) },
                        selected = draft.selected,
                        paused = false,
                    )
                    val created = withContext(io) { storage.createFiles(skeleton, null) }
                    try {
                        val pfds = withContext(io) { storage.openFiles(created) }
                        try {
                            val descriptors = descriptorsFor(created, pfds, draft.selected)
                            withContext(io) { client.configure(draft.engineId, descriptors, draft.selected) }
                        } finally {
                            pfds.forEach { runCatching { it?.close() } }
                        }
                    } catch (t: Throwable) {
                        runCatching { withContext(io) { storage.deleteFiles(created) } }
                        throw t
                    }
                    val entry = skeleton.copy(files = created)
                    _ui.update {
                        it.copy(
                            library = listOf(entry) + it.library.filter { e -> e.key != key },
                            prepare = null,
                            screen = Screen.Library,
                            addSheetOpen = false,
                            magnetDraft = "",
                        )
                    }
                    persist()
                    refreshStorageStats()
                    syncService()
                } catch (t: Throwable) {
                    if (t is CancellationException) throw t
                    setPrepareError(readableError(t))
                }
            }
        }
    }

    fun pause(entry: DownloadEntry) {
        scope.launch {
            val id = entry.engineId ?: return@launch
            try {
                withContext(io) { client.pause(id) }
                patch(entry.key) { it.copy(paused = true, error = null) }
                persist()
                syncService()
            } catch (t: Throwable) {
                patch(entry.key) { it.copy(error = readableError(t)) }
            }
        }
    }

    fun resume(entry: DownloadEntry) {
        scope.launch {
            try {
                val current = _ui.value.library.find { it.key == entry.key } ?: entry
                if (current.engineId != null) {
                    withContext(io) { client.resume(current.engineId) }
                    patch(current.key) { it.copy(paused = false, error = null) }
                } else {
                    restoreEntry(current, startPaused = false)
                }
                persist()
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                patch(entry.key) { it.copy(error = readableError(t)) }
            }
        }
    }

    fun stop(entry: DownloadEntry) {
        scope.launch {
            entry.engineId?.let { id -> runCatching { withContext(io) { client.remove(id, false) } } }
            patch(entry.key) { it.copy(engineId = null, paused = true, status = null) }
            persist()
            syncService()
        }
    }

    fun play(entry: DownloadEntry, fileIndex: Int? = null) {
        scope.launch {
            try {
                var current = _ui.value.library.find { it.key == entry.key } ?: entry
                val targetIndex = fileIndex ?: pickPlayIndex(current)
                check(targetIndex in current.selected) { "This file was not selected for download." }
                completedPlayFile(current, targetIndex)?.let { file ->
                    _events.emit(UiEvent.OpenContent(file.uri!!, mimeFor(file.name), file.name))
                    return@launch
                }
                if (current.files.none { it.index in current.selected && it.uri != null }) {
                    error("Start this download before playing.")
                }
                if (current.engineId == null) {
                    restoreEntry(current, startPaused = false)
                    current = _ui.value.library.find { it.key == entry.key } ?: current
                }
                val id = current.engineId ?: error("Could not start this torrent.")
                val status = withContext(io) { client.torrent(id) }
                if (status.paused || current.paused) {
                    withContext(io) { client.resume(id) }
                    patch(current.key) { it.copy(paused = false) }
                }
                val info = withContext(io) { client.play(id, targetIndex) }
                _events.emit(UiEvent.PlayStream(info))
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                val msg = readableError(t)
                patch(entry.key) { it.copy(error = msg) }
                _ui.update { it.copy(error = msg) }
            }
        }
    }

    fun open(entry: DownloadEntry) {
        if (entry.selected.size > 1) showFiles(entry) else play(entry)
    }

    fun showFiles(entry: DownloadEntry) {
        _ui.update { it.copy(screen = Screen.Files, openTorrentKey = entry.key, error = null) }
    }

    fun closeFiles() {
        _ui.update { it.copy(screen = Screen.Library, openTorrentKey = null, error = null) }
    }

    fun retry(entry: DownloadEntry) {
        scope.launch {
            patch(entry.key) { it.copy(error = null) }
            try {
                val current = _ui.value.library.find { it.key == entry.key } ?: entry
                restoreEntry(current, startPaused = current.paused)
                persist()
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                patch(entry.key) { it.copy(error = readableError(t)) }
            }
        }
    }

    fun deleteKeep() {
        val req = _ui.value.deleteRequest ?: return
        scope.launch {
            val entry = _ui.value.library.find { it.key == req.key }
            entry?.engineId?.let { id -> runCatching { withContext(io) { client.remove(id, false) } } }
            _ui.update { it.copy(library = it.library.filter { e -> e.key != req.key }, deleteRequest = null) }
            persist()
            refreshStorageStats()
            syncService()
        }
    }

    fun deleteErase() {
        val req = _ui.value.deleteRequest ?: return
        scope.launch {
            val entry = _ui.value.library.find { it.key == req.key } ?: run {
                _ui.update { it.copy(deleteRequest = null) }
                return@launch
            }
            try {
                entry.engineId?.let { id -> withContext(io) { client.remove(id, false) } }
                patch(entry.key) { it.copy(engineId = null, paused = true, status = null) }
                withContext(io) { storage.deleteFiles(entry.files) }
                _ui.update { it.copy(library = it.library.filter { e -> e.key != req.key }, deleteRequest = null) }
                persist()
                refreshStorageStats()
                syncService()
            } catch (t: Throwable) {
                patch(entry.key) {
                    it.copy(
                        error = "Could not delete the downloaded files. Check folder access or delete them from your file manager, then retry.",
                    )
                }
                _ui.update { it.copy(deleteRequest = null) }
                persist()
            }
        }
    }

    fun confirmClearAll() {
        scope.launch {
            val next = _ui.value.library.map { entry ->
                try {
                    entry.engineId?.let { id -> withContext(io) { client.remove(id, false) } }
                    patch(entry.key) { it.copy(engineId = null, paused = true, status = null) }
                    withContext(io) { storage.deleteFiles(entry.files) }
                    entry.copy(
                        files = entry.files.map { it.copy(uri = null, progress = 0.0) },
                        engineId = null,
                        paused = true,
                        status = null,
                        error = "Downloaded files were deleted.",
                    )
                } catch (_: Throwable) {
                    entry.copy(
                        error = "Could not delete some files. Check folder access or delete them from your file manager.",
                    )
                }
            }
            _ui.update { it.copy(library = next, clearAllConfirm = false, message = "Downloaded files were removed.") }
            persist()
            refreshStorageStats()
            syncService()
        }
    }

    fun cleanupCache() {
        scope.launch {
            try {
                withContext(io) {
                    storage.clearCache()
                    if (storage.legacyBytes > 0) storage.clearLegacy()
                }
                refreshStorageStats()
                _ui.update { it.copy(message = "Temporary files were removed.") }
            } catch (t: Throwable) {
                _ui.update { it.copy(error = readableError(t)) }
            }
        }
    }

    fun pauseFromNotification() {
        scope.launch {
            val entries = _ui.value.library.filter { it.engineId != null && !it.paused && !it.complete }
            for (entry in entries) {
                val id = entry.engineId ?: continue
                runCatching { withContext(io) { client.pause(id) } }
                patch(entry.key) { it.copy(paused = true, error = null) }
            }
            persist()
            syncService()
        }
    }

    fun resumeFromNotification() {
        scope.launch {
            val entries = _ui.value.library.filter { it.paused && !it.complete }
            for (entry in entries) {
                try {
                    if (entry.engineId != null) {
                        withContext(io) { client.resume(entry.engineId) }
                        patch(entry.key) { it.copy(paused = false, error = null) }
                    } else {
                        restoreEntry(entry, startPaused = false)
                    }
                } catch (t: Throwable) {
                    if (t is CancellationException) throw t
                    patch(entry.key) { it.copy(error = readableError(t)) }
                }
            }
            persist()
            syncService()
        }
    }

    fun stopFromNotification() {
        scope.launch {
            val entries = _ui.value.library.filter { it.engineId != null }
            for (entry in entries) {
                val id = entry.engineId ?: continue
                runCatching { withContext(io) { client.remove(id, false) } }
            }
            _ui.update { state ->
                state.copy(
                    library = state.library.map { entry ->
                        if (entry.engineId == null) entry
                        else entry.copy(engineId = null, paused = true, status = null)
                    },
                )
            }
            persist()
        }
    }

    fun refreshStorageStats() {
        if (storageStatsJob?.isActive == true) return
        storageStatsJob = scope.launch {
            val stats = withContext(io) {
                Triple(storage.freeBytes, storage.cacheBytes, storage.legacyBytes)
            }
            _ui.update {
                it.copy(
                    freeBytes = stats.first,
                    managedBytes = storage.managedBytes(it.library),
                    cacheBytes = stats.second,
                    legacyBytes = stats.third,
                )
            }
        }
    }

    private suspend fun addTorrent(torrentId: String, torrentData: String? = null, commit: Boolean = true) {
        try {
            val addedId = mutex.withLock {
                val prev = _ui.value.prepare
                if (prev != null && prev.source == torrentId && torrentData == null) {
                    if (commit) {
                        _ui.update {
                            it.copy(
                                prepare = prev.copy(committed = true, error = null),
                                screen = Screen.Prepare,
                                addSheetOpen = false,
                                error = null,
                            )
                        }
                    }
                    return@withLock prev.engineId
                }
                if (prev != null) {
                    runCatching { withContext(io) { client.remove(prev.engineId, true) } }
                    _ui.update { it.copy(prepare = null) }
                }
                val hash = infoHashFromMagnet(torrentId)
                if (hash != null && _ui.value.library.any { it.key.equals(hash, true) }) {
                    _ui.update {
                        it.copy(
                            addSheetOpen = false,
                            screen = Screen.Library,
                            message = "Already in your library.",
                            error = null,
                        )
                    }
                    return
                }
                if (!_ui.value.engineReady) waitForEngine()
                if (!_ui.value.engineReady) {
                    _ui.update {
                        it.copy(
                            error = "The download engine is not running. Close Torrent Player and open it again.",
                            addSheetOpen = if (commit) false else it.addSheetOpen,
                        )
                    }
                    return
                }
                val added = withContext(io) { client.add(torrentId, prepare = true, torrentData = torrentData) }
                _ui.update {
                    it.copy(
                        prepare = newDraft(added.id, torrentId, null, committed = commit),
                        screen = if (commit) Screen.Prepare else it.screen,
                        addSheetOpen = if (commit) false else it.addSheetOpen,
                        error = null,
                        magnetDraft = if (commit) "" else it.magnetDraft,
                    )
                }
                added.id
            }
            waitReady(addedId)
        } catch (t: Throwable) {
            if (t is CancellationException) throw t
            val msg = readableError(t)
            _ui.update { state ->
                if (state.prepare != null && (state.screen == Screen.Prepare || state.prepare.committed)) {
                    state.copy(prepare = state.prepare.copy(error = msg, busy = false))
                } else {
                    state.copy(error = msg)
                }
            }
        }
    }

    private fun newDraft(engineId: String, source: String, torrent: TorrentStatus?, committed: Boolean = false): PrepareDraft {
        return PrepareDraft(
            engineId = engineId,
            source = source,
            torrent = torrent,
            selected = torrent?.files?.let { defaultVideoSelection(it) } ?: emptySet(),
            committed = committed,
        )
    }

    private suspend fun waitReady(id: String, timeoutMs: Long = 90_000, forPrepare: Boolean = true): TorrentStatus {
        val start = System.currentTimeMillis()
        var last: TorrentStatus? = null
        while (System.currentTimeMillis() - start < timeoutMs) {
            if (forPrepare && _ui.value.prepare?.engineId != id) throw CancellationException("prefetch cleared")
            val t = withContext(io) { client.torrent(id) }
            last = t
            if (t.error != null) error(readableError(t.error))
            applyPrepareStatus(t)
            if (t.ready) {
                val selected = _ui.value.prepare?.takeIf { it.engineId == id }?.selected
                if (selected != null) runCatching { withContext(io) { client.select(id, selected) } }
                return t
            }
            delay(500)
        }
        val peers = last?.numPeers ?: 0
        error(
            if (peers == 0) "No peers found. The torrent may have no seeds right now. Try again later."
            else "Timed out waiting for torrent metadata. Check your connection or try again.",
        )
    }

    private fun dropUncommittedPrefetch(exceptSource: String? = null) {
        val draft = _ui.value.prepare ?: return
        if (draft.committed) return
        if (exceptSource != null && draft.source == exceptSource) return
        cancelPrepare()
    }

    private fun syncPrepareSelection() {
        val draft = _ui.value.prepare ?: return
        if (draft.torrent?.ready != true) return
        val id = draft.engineId
        val selected = draft.selected
        scope.launch {
            runCatching { withContext(io) { client.select(id, selected) } }
        }
    }

    private fun applyPrepareStatus(t: TorrentStatus) {
        _ui.update { state ->
            val draft = state.prepare ?: return@update state
            if (draft.engineId != t.id) return@update state
            val selected = when {
                draft.torrent?.ready == true -> draft.selected
                t.files.isNotEmpty() -> defaultVideoSelection(t.files)
                else -> emptySet()
            }
            state.copy(prepare = draft.copy(torrent = t, selected = selected))
        }
    }

    private suspend fun restoreAll(entries: List<DownloadEntry>) {
        for (entry in entries) {
            if (entry.complete) continue
            restoreEntry(entry, startPaused = entry.paused)
        }
    }

    private suspend fun restoreEntry(entry: DownloadEntry, startPaused: Boolean) {
        val current = _ui.value.library.find { it.key == entry.key } ?: entry
        val hasFiles = current.files.any { it.index in current.selected && it.uri != null }
        if (!hasFiles) return
        if (current.metadata.isBlank() && current.source.isBlank()) {
            patch(current.key) { it.copy(error = "This download has no torrent metadata to restore.") }
            return
        }
        val missing = withContext(io) {
            current.files.filter { it.index in current.selected && it.uri != null }
                .filter { !storage.fileAccessible(it) }
        }
        if (missing.isNotEmpty()) {
            patch(current.key) {
                it.copy(
                    error = "A downloaded file is missing or folder access was revoked. Re-grant folder access, then retry. You can also delete leftover files from your file manager.",
                )
            }
            return
        }
        if (!_ui.value.engineReady) {
            patch(current.key) { it.copy(error = "The download engine is not running. Close Torrent Player and open it again.") }
            return
        }
        try {
            val torrentId = current.source.ifBlank { current.key }
            val data = current.metadata.takeIf { it.isNotBlank() }
            val added = withContext(io) { client.add(torrentId, prepare = true, torrentData = data) }
            waitReady(added.id, timeoutMs = 30_000, forPrepare = false)
            val pfds = withContext(io) { storage.openFiles(current.files) }
            try {
                val descriptors = descriptorsFor(current.files, pfds, current.selected)
                withContext(io) { client.configure(added.id, descriptors, current.selected) }
            } catch (e: EngineException) {
                if (e.message?.contains("already configured", ignoreCase = true) != true) throw e
            } finally {
                pfds.forEach { runCatching { it?.close() } }
            }
            if (startPaused) withContext(io) { client.pause(added.id) }
            patch(current.key) { it.copy(engineId = added.id, paused = startPaused, error = null) }
        } catch (t: Throwable) {
            if (t is CancellationException) throw t
            patch(current.key) { it.copy(error = readableError(t), engineId = null) }
        }
    }

    private suspend fun pollLoop() {
        while (true) {
            val snapshot = _ui.value
            val active = snapshot.library.any { it.engineId != null && !it.complete && !it.paused } ||
                snapshot.prepare != null
            delay(
                when {
                    snapshot.prepare != null && snapshot.prepare.torrent?.ready != true -> 500
                    active -> 750
                    else -> 1500
                },
            )
            val state = _ui.value
            if (!state.engineReady) continue
            val ids = buildList {
                state.library.forEach { e -> e.engineId?.let(::add) }
                if (prefetchJob?.isActive != true) state.prepare?.engineId?.let(::add)
            }.distinct()
            var changed = false
            var completed = false
            for (id in ids) {
                try {
                    val t = withContext(io) { client.torrent(id) }
                    val before = _ui.value.library.find { it.engineId == id }
                    applyStatus(t)
                    applyPrepareStatus(t)
                    val after = _ui.value.library.find { it.engineId == id }
                    if (before != null && after != null && before.downloaded != after.downloaded) changed = true
                    if (after?.complete == true && before?.complete != true) completed = true
                } catch (e: EngineException) {
                    if (!state.restoring) markMissingEngine(id, e)
                } catch (_: Throwable) {
                }
            }
            val now = System.currentTimeMillis()
            if ((changed && now - lastPersistAt > 10_000) || completed) {
                persist()
                lastPersistAt = now
            }
            if (state.screen == Screen.Settings) refreshStorageStats()
            syncService()
        }
    }

    private fun applyStatus(t: TorrentStatus) {
        _ui.update { state ->
            val idx = state.library.indexOfFirst { it.engineId == t.id }
            if (idx < 0) return@update state
            val entry = state.library[idx]
            val files = entry.files.map { f ->
                val tf = t.files.find { it.index == f.index }
                if (tf != null) f.copy(progress = tf.progress) else f
            }
            val next = entry.copy(
                files = files,
                status = t,
                paused = t.paused,
                error = t.error?.let { readableError(it) },
                title = t.name?.takeIf { it.isNotBlank() } ?: entry.title,
            )
            state.copy(library = state.library.toMutableList().also { it[idx] = next })
        }
    }

    private fun markMissingEngine(id: String, error: EngineException) {
        val msg = readableError(error)
        _ui.update { state ->
            state.copy(
                library = state.library.map { entry ->
                    if (entry.engineId != id) entry
                    else entry.copy(
                        engineId = null,
                        paused = true,
                        status = null,
                        error = if (entry.complete) null else msg,
                    )
                },
            )
        }
    }

    private fun descriptorsFor(
        files: List<SavedFile>,
        pfds: List<ParcelFileDescriptor?>,
        selected: Set<Int>,
    ): List<Int?> {
        val max = files.maxOfOrNull { it.index } ?: -1
        val fds = arrayOfNulls<Int>(max + 1)
        files.forEachIndexed { i, file ->
            fds[file.index] = if (file.index in selected) pfds.getOrNull(i)?.fd else null
        }
        return fds.toList()
    }

    private fun patch(key: String, transform: (DownloadEntry) -> DownloadEntry) {
        _ui.update { state ->
            state.copy(library = state.library.map { if (it.key == key) transform(it) else it })
        }
    }

    private fun setPrepareError(message: String) {
        _ui.update { it.copy(prepare = it.prepare?.copy(busy = false, error = message)) }
    }

    private suspend fun persist() {
        val snapshot = _ui.value.library
        withContext(io) { runCatching { storage.save(snapshot) } }
    }

    private fun syncService() {
        val live = _ui.value.library.filter { it.engineId != null && !it.complete }
        if (live.isEmpty()) {
            PlayService.stop(app)
            lastServiceUpdate = 0L
            return
        }
        val downloading = live.filter { !it.paused }
        val total = live.sumOf { it.total }
        val got = live.sumOf { it.downloaded }
        val progress = if (total <= 0L) 0 else ((got.toDouble() / total) * 100).toInt().coerceIn(0, 100)
        val title = if (live.size == 1) live.first().title else "${live.size} downloads"
        val text = if (downloading.isEmpty()) {
            "Paused"
        } else {
            "${formatBytes(got)} / ${formatBytes(total)}  ·  ${formatSpeed(downloading.sumOf { it.status?.downloadSpeed ?: 0L })}"
        }
        val now = android.os.SystemClock.elapsedRealtime()
        if (now - lastServiceUpdate >= 1000) {
            PlayService.start(app, title, progress, downloading.isEmpty(), text)
            lastServiceUpdate = now
        }
    }

    private suspend fun waitForEngine() {
        repeat(200) {
            try {
                withContext(io) { client.stats() }
                _ui.update { it.copy(engineReady = true, statusLine = "Ready") }
                return
            } catch (_: Throwable) {
                delay(100)
            }
        }
        _ui.update {
            it.copy(
                engineReady = false,
                error = "The download engine is not running. Close Torrent Player and open it again.",
                statusLine = "Engine down",
            )
        }
    }
}

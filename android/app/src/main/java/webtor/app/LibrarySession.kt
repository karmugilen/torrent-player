package webtor.app

import android.app.Application
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import webtor.core.EngineClient
import webtor.core.EngineException
import webtor.core.TorrentStatus

class LibrarySession(
    private val app: Application,
    private val client: EngineClient,
    private val engineHost: EngineHost,
) {
    private val storage = DownloadStorage(app)
    private val settings = app.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val io = Dispatchers.IO
    private val mutex = Mutex()
    private val persistenceMutex = Mutex()
    private val started = AtomicBoolean(false)
    private val initialized = CompletableDeferred<Unit>()
    private val isShuttingDown = AtomicBoolean(false)
    private var serviceSuppressed = false
    private val recentlyInvalidatedIds = java.util.Collections.newSetFromMap(
        java.util.concurrent.ConcurrentHashMap<String, Boolean>()
    )
    private val pendingMaterializations = java.util.Collections.newSetFromMap(
        java.util.concurrent.ConcurrentHashMap<String, Boolean>()
    )
    private var lastPersistAt = 0L
    private var debounceJob: Job? = null
    private var prefetchJob: Job? = null
    private var storageStatsJob: Job? = null
    private var eventJob: Job? = null
    private var lastServiceUpdate = 0L
    private var draftGeneration = 0L
    private var commitInProgress = false
    private val retryStates = mutableMapOf<String, RetryState>()
    private val retryJobs = mutableMapOf<String, Job>()
    private var networkConnected = true
    private var networkWifi = true
    private var networkMetered = false
    @Volatile private var shutdownComplete = false

    private val _ui = MutableStateFlow(UiState())
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    private val _events = Channel<UiEvent>(Channel.BUFFERED)
    val events = _events.receiveAsFlow()

    fun onForeground() {
        reopenAfterShutdown()
    }

    private fun shutdownInProgress(): Boolean = isShuttingDown.get() && !shutdownComplete

    private fun restartEventsIfNeeded() {
        if (eventJob?.isActive == true) return
        eventJob?.cancel()
        eventJob = scope.launch { eventLoop() }
    }

    private fun reopenAfterShutdown(): Boolean {
        if (isShuttingDown.get()) {
            if (!shutdownComplete) return false
            isShuttingDown.set(false)
            shutdownComplete = false
        }
        restartEventsIfNeeded()
        return true
    }

    private fun beginUserWork(): Boolean {
        if (!reopenAfterShutdown()) return false
        serviceSuppressed = false
        settings.edit().putBoolean("serviceSuppressed", false).apply()
        return true
    }

    private fun CoroutineScope.launchCommand(block: suspend () -> Unit) = launch {
        initialized.await()
        mutex.withLock { block() }
    }

    fun start() {
        if (!started.compareAndSet(false, true)) return
        serviceSuppressed = settings.getBoolean("serviceSuppressed", false)
        // Reset previous custom connection tuning once; subsequent user changes persist.
        if (!settings.getBoolean("peerDefaultsV2", false)) {
            settings.edit().putInt("maxPeers", 55).putBoolean("peerDefaultsV2", true).apply()
        }
        refreshStorageStats()
        scope.launch {
            try {
                val loaded = withContext(io) { storage.load() }.map {
                    val staysPaused = it.paused || serviceSuppressed
                    it.copy(
                        engineId = null,
                        paused = staysPaused,
                        lifecycleState = when {
                            it.complete -> EntryLifecycleState.COMPLETED
                            staysPaused -> EntryLifecycleState.PAUSED
                            else -> EntryLifecycleState.PREPARING
                        },
                    )
                }
                _ui.update {
                    it.copy(
                        library = loaded,
                        loadWarning = storage.loadWarning,
                        maxPeers = settings.getInt("maxPeers", 55).coerceIn(8, 80),
                        restoring = loaded.isNotEmpty(),
                        statusLine = if (loaded.isEmpty()) it.statusLine else "Restoring downloads…",
                        darkTheme = settings.getBoolean("darkTheme", true),
                        networkPolicy = savedNetworkPolicy(),
                    )
                }
                // Publish the local library before slow metadata and descriptor
                // recovery so unrelated commands remain responsive.
                initialized.complete(Unit)
                settings.edit().also { editor ->
                    editor.remove("playerPackage").remove("playerActivity")
                    editor.remove("downloadRateBytesSec").remove("uploadRateBytesSec")
                    LEGACY_DOWNLOAD_QUEUE_PREF_KEYS.forEach(editor::remove)
                }.apply()
                refreshStorageStats()
                waitForEngine()
                if (_ui.value.engineReady) {
                    runCatching { withContext(io) { client.setMaxPeers(_ui.value.maxPeers) } }
                    runCatching { withContext(io) { client.setPayloadTransfersAllowed(_ui.value.networkAllowed) } }
                    restoreAll(loaded)
                }
                mutex.withLock { reconcileTransferState() }
                _ui.update { it.copy(restoring = false, statusLine = if (it.engineReady) "Ready" else it.statusLine) }
                persist()
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t), restoring = false) }
                initialized.complete(Unit)
            }
        }
        eventJob = scope.launch { eventLoop() }
    }

    fun setMagnet(value: String) {
        if (commitInProgress || !reopenAfterShutdown()) return
        draftGeneration++
        prefetchJob?.cancel()
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
    fun openAddSheet() {
        if (!beginUserWork()) return
        _ui.update { it.copy(addSheetOpen = true, error = null) }
    }
    fun closeAddSheet() {
        if (commitInProgress) return
        draftGeneration++
        debounceJob?.cancel()
        _ui.update { it.copy(addSheetOpen = false, error = null) }
        dropUncommittedPrefetch()
    }
    fun openSettings() {
        refreshStorageStats()
        _ui.update { it.copy(screen = Screen.Settings, error = null) }
    }

    fun setDarkTheme(dark: Boolean) {
        settings.edit().putBoolean("darkTheme", dark).apply()
        _ui.update { it.copy(darkTheme = dark) }
    }
    fun setNetworkPolicy(policy: TransferNetworkPolicy) {
        settings.edit().putString("networkPolicy", policy.name).apply()
        updateNetworkState(networkConnected, networkWifi, networkMetered, policy)
    }

    fun updateNetworkState(connected: Boolean, wifi: Boolean, metered: Boolean) =
        updateNetworkState(connected, wifi, metered, _ui.value.networkPolicy)

    private fun updateNetworkState(connected: Boolean, wifi: Boolean, metered: Boolean, policy: TransferNetworkPolicy) {
        val wasAllowed = _ui.value.networkAllowed
        networkConnected = connected
        networkWifi = wifi
        networkMetered = metered
        val allowed = transferAllowed(policy, connected, wifi, metered)
        _ui.update { it.copy(networkPolicy = policy, networkAllowed = allowed, networkReason = if (allowed) null else "Waiting for ${policy.name.lowercase().replace('_', ' ')} network") }
        scope.launch {
            if (_ui.value.engineReady) runCatching { withContext(io) { client.setPayloadTransfersAllowed(allowed) } }
            mutex.withLock {
                reconcileTransferState()
                if (!wasAllowed && allowed) advanceRetriesForNetworkRecovery()
            }
        }
    }

    fun persistFolder(uri: android.net.Uri) { runCatching { storage.persistFolder(uri, "Selected folder") }.onSuccess { _ui.update { it.copy(message = "Download folder updated") } }.onFailure { showError("Folder access is unavailable") } }

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
    fun focusEntryFile(key: String, index: Int?) {
        scope.launchCommand {
            val e = _ui.value.library.find { it.key == key } ?: return@launchCommand
            val id = e.engineId ?: return@launchCommand
            runCatching { withContext(io) { client.focus(id, index) } }.onSuccess {
                patch(key) { it.copy(focusedFile = index) }; persist()
            }
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
    fun pickTorrent() { _events.trySend(UiEvent.PickTorrent) }
    fun exportDiagnostics() { _ui.update { it.copy(diagnosticsPreview = buildDiagnosticsReport(it.library, "1.4.5")) } }
    fun closeDiagnostics() { _ui.update { it.copy(diagnosticsPreview = null) } }
    fun shareDiagnostics() { _ui.value.diagnosticsPreview?.let { _events.trySend(UiEvent.ShareText(it)) } }

    fun addCurrent() {
        if (!beginUserWork()) return
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
                    prepare = prep.copy(committed = false, error = null),
                    addSheetOpen = true,
                    error = null,
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
        if (commitInProgress || !beginUserWork()) return
        val id = torrentId.trim()
        if (id.isEmpty()) {
            _ui.update { it.copy(error = "Paste a magnet or torrent URL.") }
            return
        }
        debounceJob?.cancel()
        prefetchJob?.cancel()
        draftGeneration++
        _ui.update { it.copy(addSheetOpen = true, magnetDraft = id, error = null) }
        prefetchJob = scope.launch { addTorrent(id, commit = false) }
    }

    fun openTorrent(uri: Uri) {
        if (commitInProgress || !beginUserWork()) return
        debounceJob?.cancel()
        prefetchJob?.cancel()
        val generation = ++draftGeneration
        _ui.update { it.copy(addSheetOpen = true, magnetDraft = "", error = null) }
        prefetchJob = scope.launch {
            try {
                val file = withContext(io) {
                    val dest = File.createTempFile("import-", ".torrent", app.cacheDir)
                    try {
                        app.contentResolver.openInputStream(uri)?.use { input ->
                            dest.outputStream().use { output ->
                                val buffer = ByteArray(8192)
                                var total = 0L
                                while (true) {
                                    val count = input.read(buffer)
                                    if (count == -1) break
                                    total += count
                                    check(total <= 8 * 1024 * 1024) { "Torrent files must be smaller than 8 MB." }
                                    output.write(buffer, 0, count)
                                }
                            }
                        } ?: error("Cannot open torrent file")
                        dest
                    } catch (t: Throwable) {
                        dest.delete()
                        throw t
                    }
                }
                try {
                    if (generation == draftGeneration) addTorrent(file.absolutePath, commit = false)
                } finally {
                    file.delete()
                }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t), addSheetOpen = true) }
            }
        }
    }

    fun cancelPrepare() {
        if (commitInProgress) return
        draftGeneration++
        debounceJob?.cancel()
        prefetchJob?.cancel()
        val id = _ui.value.prepare?.engineId
        _ui.update { it.copy(prepare = null, screen = Screen.Library, error = null) }
        scope.launch {
            mutex.withLock {
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

    /** Change selection for an already committed torrent from its details page. */
    fun toggleDownloadFile(entryKey: String, index: Int) {
        scope.launchCommand {
            if (isShuttingDown.get()) return@launchCommand
            val current = _ui.value.library.find { it.key == entryKey } ?: return@launchCommand
            if (current.isDeleting || current.controlsBusy() || !current.metadataReady) return@launchCommand
            val file = current.files.find { it.index == index } ?: return@launchCommand
            val selecting = index !in current.selected
            val next = if (selecting) current.selected + index else current.selected - index
            val generation = current.generation + 1
            patch(entryKey) { it.copy(generation = generation, error = null) }
            var addedFile: SavedFile? = null
            try {
                val effectiveFiles = if (selecting && file.uri.isNullOrBlank()) {
                    // Allocate the destination only when the user requests the
                    // file. The operation is reversible if /select fails.
                    withContext(io) { storage.createAdditionalFile(current, file) }.also { addedFile = it }
                    current.files.map { if (it.index == index) addedFile!! else it }
                } else current.files
                val id = current.engineId
                if (id != null) {
                    val pfds = withContext(io) { storage.openFiles(effectiveFiles, next) }
                    try {
                        val descriptors = descriptorsFor(effectiveFiles, pfds, next)
                        withContext(io) { client.select(id, next, descriptors) }
                    } finally {
                        pfds.forEach { runCatching { it?.close() } }
                    }
                }
                patch(entryKey) {
                    if (it.generation != generation) it
                    else {
                        val updated = it.copy(
                            files = if (addedFile == null) it.files else it.files.map { f -> if (f.index == index) addedFile!! else f },
                            selected = next,
                            error = null,
                        )
                        updated.copy(
                            lifecycleState = when {
                                updated.paused -> EntryLifecycleState.PAUSED
                                next.isEmpty() -> EntryLifecycleState.STOPPED
                                updated.complete -> EntryLifecycleState.COMPLETED
                                else -> EntryLifecycleState.DOWNLOADING
                            },
                        )
                    }
                }
                persist()
                syncService(force = true)
                reconcileTransferState()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                addedFile?.let { runCatching { withContext(io) { storage.deleteFiles(listOf(it)) } } }
                patch(entryKey) { it.copy(error = readableError(t)) }
            }
        }
    }

    fun startDownload() = commitPreparedDownload()

    private suspend fun releasePreparedEngine(id: String) {
        if (_ui.value.library.any { it.engineId == id }) return
        withContext(io + NonCancellable) {
            val status = try { client.torrent(id) } catch (e: EngineException) {
                if (e.statusCode == 404) return@withContext
                throw e
            }
            if (!status.configured) client.remove(id, true)
        }
    }

    private fun commitPreparedDownload() {
        scope.launch {
            mutex.withLock {
                if (commitInProgress || !beginUserWork()) return@withLock
                val draft = _ui.value.prepare ?: return@withLock
                val torrent = draft.torrent
                if (torrent?.ready != true) {
                    val source = draft.source.trim()
                    val key = infoHashFromMagnet(source) ?: draft.engineId
                    if (_ui.value.library.any { it.key.equals(key, true) || it.engineId == draft.engineId }) {
                        setPrepareError("This torrent is already in your library.")
                        return@withLock
                    }
                    val pending = DownloadEntry(
                        key = key,
                        title = source.substringAfter("dn=", "").substringBefore('&')
                            .ifBlank { key.take(12) },
                        source = source,
                        metadata = "",
                        engineId = draft.engineId,
                        destination = "Downloads/Webtor",
                        files = emptyList(),
                        selected = emptySet(),
                        paused = false,
                        status = torrent,
                        lifecycleState = EntryLifecycleState.DOWNLOADING,
                        metadataReady = false,
                        autoSelect = true,
                    )
                    _ui.update {
                        it.copy(
                            library = listOf(pending) + it.library,
                            prepare = null,
                            addSheetOpen = false,
                            screen = Screen.Library,
                            magnetDraft = "",
                            message = null,
                        )
                    }
                    // The draft prefetch may be in waitReady; it must not own or
                    // remove this now committed engine record.
                    prefetchJob?.cancel()
                    persist()
                    syncService(force = true)
                    reconcileTransferState()
                    return@withLock
                }
                if (draft.selected.isEmpty()) {
                    setPrepareError("Select at least one file to download.")
                    return@withLock
                }
                if (_ui.value.library.any { it.key.equals(torrent.infoHash, true) || it.engineId == draft.engineId }) {
                    setPrepareError("This torrent is already in your library.")
                    return@withLock
                }
                if (Build.VERSION.SDK_INT < 29) {
                    setPrepareError("Torrent Player needs Android 10 or newer to save into Downloads.")
                    return@withLock
                }
                commitInProgress = true
                val key = torrent.infoHash ?: draft.engineId
                val provisional = DownloadEntry(
                    key = key,
                    title = torrent.name ?: key,
                    source = torrent.magnetURI ?: draft.source,
                    metadata = "",
                    engineId = draft.engineId,
                    destination = "Downloads/Webtor",
                    files = torrent.files.map {
                        SavedFile(
                            it.index, it.name, it.path, it.length,
                            progress = it.progress,
                            verifiedBytes = it.verifiedBytes.coerceIn(0L, it.length.coerceAtLeast(0L)),
                        )
                    },
                    selected = draft.selected,
                    paused = false,
                    status = torrent,
                    lifecycleState = EntryLifecycleState.PREPARING,
                    metadataReady = true,
                )
                // Render the accepted download before any storage or engine I/O. The
                // provisional entry is not persisted until configuration succeeds.
                _ui.update {
                    it.copy(
                        library = listOf(provisional) + it.library,
                        prepare = draft.copy(busy = true, error = null),
                        addSheetOpen = false,
                        screen = Screen.Library,
                    )
                }
                _events.trySend(UiEvent.RequestNotifications)
                try {
                    val needed = torrent.files.filter { it.index in draft.selected }.sumOf { it.length }
                    val free = withContext(io) { storage.freeBytes }
                    check(needed <= free) {
                        "Not enough storage. This download needs ${formatBytes(needed)}; ${formatBytes(free)} is available."
                    }
                    val metadata = withContext(io) { client.metadata(draft.engineId) }
                    val skeleton = provisional.copy(metadata = metadata)
                    val createdResult = withContext(io) { storage.createFiles(skeleton, storage.folder) }
                    val created = createdResult.files
                    try {
                        val pfds = withContext(io) { storage.openFiles(created, draft.selected) }
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
                    val entry = skeleton.copy(
                        files = created,
                        downloadGroupUri = createdResult.groupUri,
                        lifecycleState = EntryLifecycleState.DOWNLOADING,
                    )
                    serviceSuppressed = false
                    settings.edit().putBoolean("serviceSuppressed", false).apply()
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
                    syncService(force = true)
                    reconcileTransferState()
                } catch (t: Throwable) {
                    _ui.update {
                        it.copy(
                            library = it.library.filterNot { entry -> entry.key == key },
                            addSheetOpen = true,
                            prepare = draft.copy(busy = false, error = readableError(t)),
                        )
                    }
                    if (t is CancellationException) throw t
                } finally {
                    commitInProgress = false
                }
            }
        }
    }


    fun pause(entry: DownloadEntry) {
        scope.launchCommand {
            if (isShuttingDown.get()) return@launchCommand
            val current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
            if (current.isDeleting) return@launchCommand
            cancelRetry(current.key)
            val id = current.engineId ?: return@launchCommand
            val prevLifecycle = current.lifecycleState
            val prevPaused = current.paused
            val nextGen = current.generation + 1

            patch(current.key) {
                it.copy(
                    generation = nextGen,
                    lifecycleState = EntryLifecycleState.PAUSING,
                    error = null,
                )
            }

            try {
                withContext(io) { client.pause(id) }
                patch(current.key) {
                    if (it.generation == nextGen) {
                        it.copy(paused = true, lifecycleState = EntryLifecycleState.PAUSED, error = null)
                    } else it
                }
                persist()
                syncService(force = true)
                reconcileTransferState()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                patch(current.key) {
                    if (it.generation == nextGen) {
                        it.copy(paused = prevPaused, lifecycleState = prevLifecycle, error = readableError(t))
                    } else it
                }
                syncService(force = true)
            }
        }
    }

    /** Applies a batch command independently so one bad entry does not block the rest. */
    fun pauseAll(entries: List<DownloadEntry>) = entries.forEach { pause(it) }

    fun resume(entry: DownloadEntry) {
        scope.launchCommand {
            if (!beginUserWork()) return@launchCommand

            val current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
            if (current.isDeleting || current.complete) return@launchCommand
            if (current.metadataReady && current.selected.isEmpty()) {
                _ui.update { it.copy(message = "Select at least one file to resume this download.") }
                return@launchCommand
            }
            cancelRetry(current.key)

            val prevLifecycle = current.lifecycleState
            val prevPaused = current.paused
            val nextGen = current.generation + 1
            val restoring = current.engineId == null

            patch(current.key) {
                it.copy(
                    generation = nextGen,
                    lifecycleState = if (restoring) EntryLifecycleState.PREPARING else EntryLifecycleState.DOWNLOADING,
                    error = null,
                )
            }

            try {
                if (current.engineId != null) {
                    withContext(io) { client.resume(current.engineId) }
                    patch(current.key) {
                        if (it.generation == nextGen) {
                            it.copy(paused = false, lifecycleState = EntryLifecycleState.DOWNLOADING, error = null)
                        } else it
                    }
                } else {
                    restoreEntry(current, startPaused = false, commandGeneration = nextGen)
                }
                persist()
                refreshStorageStats()
                syncService()
                reconcileTransferState()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                patch(current.key) {
                    if (it.generation != nextGen) it
                    else if (restoring) {
                        it.copy(
                            paused = true,
                            engineId = null,
                            status = null,
                            lifecycleState = EntryLifecycleState.STOPPED,
                            error = readableError(t),
                        )
                    } else {
                        it.copy(paused = prevPaused, lifecycleState = prevLifecycle, error = readableError(t))
                    }
                }
            }
        }
    }

    /** Applies resume independently to each selected entry; failures remain on that row. */
    fun resumeAll(entries: List<DownloadEntry>) = entries.forEach { resume(it) }

    fun stop(entry: DownloadEntry) {
        scope.launchCommand {
            val current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
            if (current.isDeleting || isShuttingDown.get()) return@launchCommand
            cancelRetry(current.key)
            val id = current.engineId
            val nextGen = current.generation + 1

            if (id == null) {
                // Cancel a queued/in-flight restore as a user intent. The
                // generation prevents its eventual native add from reviving
                // this entry after the pause command has returned.
                patch(current.key) {
                    if (it.generation != current.generation) it
                    else it.copy(
                        generation = nextGen,
                        paused = true,
                        lifecycleState = EntryLifecycleState.PAUSED,
                        error = null,
                    )
                }
                persist()
                syncService(force = true)
                return@launchCommand
            }

            patch(current.key) {
                it.copy(
                    generation = nextGen,
                    lifecycleState = EntryLifecycleState.STOPPING,
                    error = null,
                )
            }

            recentlyInvalidatedIds.add(id)
            try {
                withContext(io) { client.remove(id, false) }
            } catch (t: Exception) {
                recentlyInvalidatedIds.remove(id)
                if (t is CancellationException) throw t
                patch(current.key) { it.copy(lifecycleState = current.lifecycleState, error = readableError(t)) }
                syncService(force = true)
                return@launchCommand
            }

            patch(current.key) {
                if (it.generation == nextGen) {
                    it.copy(
                        engineId = null,
                        paused = true,
                        status = null,
                        lifecycleState = if (it.complete) EntryLifecycleState.COMPLETED else EntryLifecycleState.STOPPED,
                    )
                } else it
            }
            persist()
            syncService(force = true)
        }
    }

    fun play(entry: DownloadEntry, fileIndex: Int? = null) {
        scope.launchCommand {
            if (!beginUserWork()) return@launchCommand
            try {
                var current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
                if (current.isDeleting) return@launchCommand
                val targetIndex = fileIndex ?: pickPlayIndex(current)
                check(targetIndex != null && (targetIndex in current.selected || completedPlayFile(current, targetIndex) != null)) {
                    "This file was not selected for download."
                }
                current.engineId?.let { id ->
                    val status = withContext(io) { client.torrent(id) }
                    applyStatus(status, current.key, current.generation)
                    current = _ui.value.library.find { it.key == entry.key } ?: current
                }
                if (current.checking) {
                    _ui.update { it.copy(message = "Checking saved data. Play will be available when the check finishes.") }
                    return@launchCommand
                }
                completedPlayFile(current, targetIndex)?.let { file ->
                    withContext(io) { storage.prepareForPlayback(file) }
                    _events.send(UiEvent.OpenContent(PlaybackProvider.uriFor(current, file).toString(), mimeFor(file.name), file.name))
                    return@launchCommand
                }
                if (current.files.none { it.index in current.selected && it.uri != null }) {
                    error("Start this download before playing.")
                }
                if (current.engineId == null) {
                    val nextGen = current.generation + 1
                    patch(current.key) {
                        it.copy(generation = nextGen, lifecycleState = EntryLifecycleState.PREPARING, error = null)
                    }
                    restoreEntry(current, startPaused = false, commandGeneration = nextGen)
                    current = _ui.value.library.find { it.key == entry.key } ?: current
                }
                val id = current.engineId ?: error(current.error ?: "Could not start this torrent.")
                val status = withContext(io) { client.torrent(id) }
                if (status.checking) {
                    applyStatus(status, current.key, current.generation)
                    persist()
                    syncService()
                    _ui.update { it.copy(message = "Checking saved data. Play will be available when the check finishes.") }
                    return@launchCommand
                }
                if (status.paused || current.paused) {
                    withContext(io) { client.resume(id) }
                    patch(current.key) { it.copy(paused = false) }
                }
                withContext(io) { client.play(id, targetIndex) }
                val file = current.files.first { it.index == targetIndex }
                persist()
                syncService()
                _events.send(UiEvent.OpenContent(PlaybackProvider.uriFor(current, file).toString(), mimeFor(file.name), file.name))
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

    fun openDetails(key: String) {
        closeAddSheet()
        _ui.update { it.copy(screen = Screen.Files, openTorrentKey = key, error = null) }
    }

    fun openDetails(entry: DownloadEntry) = openDetails(entry.key)

    fun closeFiles() {
        _ui.update { it.copy(screen = Screen.Library, openTorrentKey = null, error = null) }
    }

    fun retry(entry: DownloadEntry) {
        scope.launchCommand {
            if (!beginUserWork()) return@launchCommand
            val current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
            if (current.isDeleting) return@launchCommand
            cancelRetry(current.key)
            val nextGen = current.generation + 1
            patch(current.key) {
                it.copy(generation = nextGen, error = null, lifecycleState = EntryLifecycleState.PREPARING)
            }
            try {
                val startPaused = if (current.files.none { it.uri != null }) false else current.paused
                restoreEntry(current, startPaused = startPaused, commandGeneration = nextGen)
                persist()
                refreshStorageStats()
                syncService()
                reconcileTransferState()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                failRestore(current.key, nextGen, readableError(t))
            }
        }
    }

    fun deleteKeep() {
        if (isShuttingDown.get()) return
        val req = _ui.value.deleteRequest ?: return
        val target = _ui.value.library.find { it.key == req.key } ?: run {
            _ui.update { it.copy(deleteRequest = null) }
            return
        }
        cancelRetry(target.key)
        val nextGen = target.generation + 1
        // Immediately dismiss dialog and render Removing... state synchronously
        _ui.update { state ->
            state.copy(
                deleteRequest = null,
                library = state.library.map {
                    if (it.key == req.key) {
                        it.copy(
                            isDeleting = true,
                            lifecycleState = EntryLifecycleState.DELETING,
                            generation = nextGen,
                        )
                    } else it
                },
            )
        }
        syncService()

        scope.launchCommand {
            try {
                val id = target.engineId
                if (id != null) {
                    recentlyInvalidatedIds.add(id)
                    // Stream cache has nothing useful to keep in Downloads; always wipe it.
                    withContext(io) { client.remove(id, false) }
                }
                (app as? WebtorApp)?.thumbnails?.invalidate(target)
                _ui.update { state ->
                    state.copy(library = state.library.filter { it.key != target.key },
                        screen = if (state.openTorrentKey == target.key) Screen.Library else state.screen,
                        openTorrentKey = state.openTorrentKey.takeUnless { it == target.key })
                }
                persist()
                refreshStorageStats()
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                target.engineId?.let { recentlyInvalidatedIds.remove(it) }
                patch(target.key) {
                    it.copy(
                        isDeleting = false,
                        lifecycleState = EntryLifecycleState.ERROR,
                        error = readableError(t),
                    )
                }
                syncService()
            }
        }
    }

    fun deleteErase() {
        if (isShuttingDown.get()) return
        val req = _ui.value.deleteRequest ?: return
        val target = _ui.value.library.find { it.key == req.key } ?: run {
            _ui.update { it.copy(deleteRequest = null) }
            return
        }
        cancelRetry(target.key)
        val nextGen = target.generation + 1
        // Immediately dismiss dialog and render Removing... state synchronously
        _ui.update { state ->
            state.copy(
                deleteRequest = null,
                library = state.library.map {
                    if (it.key == req.key) {
                        it.copy(
                            isDeleting = true,
                            lifecycleState = EntryLifecycleState.DELETING,
                            generation = nextGen,
                        )
                    } else it
                },
            )
        }
        syncService()

        scope.launchCommand {
            var engineRemoved = target.engineId == null
            try {
                val id = target.engineId
                if (id != null) {
                    recentlyInvalidatedIds.add(id)
                    withContext(io) { client.remove(id, destroyStore = true) }
                    engineRemoved = true
                }
                withContext(io) { storage.deleteFiles(target.files) }
                (app as? WebtorApp)?.thumbnails?.invalidate(target)
                _ui.update { state ->
                    state.copy(library = state.library.filter { it.key != target.key },
                        screen = if (state.openTorrentKey == target.key) Screen.Library else state.screen,
                        openTorrentKey = state.openTorrentKey.takeUnless { it == target.key })
                }
                persist()
                refreshStorageStats()
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                if (!engineRemoved) target.engineId?.let { recentlyInvalidatedIds.remove(it) }
                patch(target.key) {
                    it.copy(
                        isDeleting = false,
                        lifecycleState = EntryLifecycleState.ERROR,
                        engineId = if (engineRemoved) null else target.engineId,
                        paused = if (engineRemoved) true else target.paused,
                        status = if (engineRemoved) null else target.status,
                        error = readableError(t),
                    )
                }
                persist()
                syncService()
            }
        }
    }

    fun confirmClearAll() {
        scope.launch {
            val next = _ui.value.library.map { entry ->
                try {
                    entry.engineId?.let { id ->
                        recentlyInvalidatedIds.add(id)
                        withContext(io) { client.remove(id, false) }
                    }
                    patch(entry.key) { it.copy(engineId = null, paused = true, status = null) }
                    withContext(io) { storage.deleteFiles(entry.files) }
                    (app as? WebtorApp)?.thumbnails?.invalidate(entry)
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

    fun refreshTransferNotification() {
        scope.launchCommand { syncService(force = true) }
    }

    fun pauseFromNotification() {
        scope.launchCommand {
            if (isShuttingDown.get()) return@launchCommand
            val targets = _ui.value.library.filter {
                it.engineId != null && !it.paused && !it.complete && !it.isDeleting
            }
            if (targets.isEmpty()) {
                syncService(force = true)
                return@launchCommand
            }

            val genMap = targets.associate { it.key to (it.generation + 1) }

            _ui.update { state ->
                state.copy(
                    library = state.library.map { entry ->
                        val nextGen = genMap[entry.key]
                        if (nextGen != null) {
                            entry.copy(
                                generation = nextGen,
                                lifecycleState = EntryLifecycleState.PAUSING,
                                error = null,
                            )
                        } else entry
                    },
                )
            }

            var failedCount = 0
            for (target in targets) {
                val id = target.engineId ?: continue
                val nextGen = genMap[target.key] ?: continue
                try {
                    withContext(io) { client.pause(id) }
                    patch(target.key) {
                        if (it.generation == nextGen) {
                            it.copy(paused = true, lifecycleState = EntryLifecycleState.PAUSED, error = null)
                        } else it
                    }
                } catch (t: Throwable) {
                    if (t is CancellationException) throw t
                    failedCount++
                    patch(target.key) {
                        if (it.generation == nextGen) {
                            it.copy(paused = target.paused, lifecycleState = target.lifecycleState, error = readableError(t))
                        } else it
                    }
                }
            }

            if (failedCount > 0) {
                _ui.update {
                    it.copy(error = "Could not pause $failedCount download(s). Check connection and retry.")
                }
            }
            persist()
            syncService(force = true)
        }
    }

    fun resumeFromNotification() {
        scope.launchCommand {
            if (!beginUserWork()) return@launchCommand

            val targets = _ui.value.library.filter { it.files.any { f -> f.index in it.selected && f.uri != null } && it.paused && !it.complete && !it.isDeleting }
            for (entry in targets) {
                val live = _ui.value.library.find { it.key == entry.key } ?: continue
                if (live.isDeleting || live.complete || live.files.none { f -> f.index in live.selected && f.uri != null }) continue
                val nextGen = live.generation + 1
                val restoring = live.engineId == null
                patch(live.key) {
                    it.copy(
                        generation = nextGen,
                        lifecycleState = if (restoring) EntryLifecycleState.PREPARING else EntryLifecycleState.DOWNLOADING,
                        error = null,
                    )
                }
                try {
                    if (live.engineId != null) {
                        withContext(io) { client.resume(live.engineId) }
                        patch(live.key) {
                            if (it.generation == nextGen) {
                                it.copy(paused = false, lifecycleState = EntryLifecycleState.DOWNLOADING, error = null)
                            } else it
                        }
                    } else {
                        restoreEntry(live, startPaused = false, commandGeneration = nextGen)
                    }
                } catch (t: Throwable) {
                    if (t is CancellationException) throw t
                    if (restoring) failRestore(live.key, nextGen, readableError(t))
                    else patch(live.key) {
                        if (it.generation == nextGen) {
                            it.copy(paused = true, lifecycleState = EntryLifecycleState.PAUSED, error = readableError(t))
                        } else it
                    }
                }
            }
            persist()
            syncService(force = true)
        }
    }

    fun stopAll(onFinished: (() -> Unit)? = null) {
        serviceSuppressed = true
        settings.edit().putBoolean("serviceSuppressed", true).apply()
        scope.launch {
            initialized.await()
            mutex.withLock {
                val targets = _ui.value.library.filter { it.engineId != null && !it.isDeleting }
                val genMap = targets.associate { it.key to (it.generation + 1) }
                _ui.update { state ->
                    state.copy(
                        library = state.library.map { entry ->
                            val nextGen = genMap[entry.key]
                            if (nextGen != null) {
                                entry.copy(
                                    generation = nextGen,
                                    lifecycleState = EntryLifecycleState.STOPPING,
                                    error = null,
                                )
                            } else entry
                        },
                    )
                }
                val stopped = mutableSetOf<String>()
                for (target in targets) {
                    val id = target.engineId ?: continue
                    recentlyInvalidatedIds.add(id)
                    try {
                        withContext(io) { client.remove(id, false) }
                        stopped += target.key
                    } catch (t: Exception) {
                        if (t is CancellationException) throw t
                        recentlyInvalidatedIds.remove(id)
                        patch(target.key) { it.copy(lifecycleState = target.lifecycleState, error = readableError(t)) }
                    }
                }
                _ui.update { state ->
                    state.copy(
                        library = state.library.map { entry ->
                            if (entry.key in stopped) {
                                entry.copy(
                                    engineId = null,
                                    paused = true,
                                    status = null,
                                    lifecycleState = if (entry.complete) EntryLifecycleState.COMPLETED else EntryLifecycleState.STOPPED,
                                )
                            } else entry
                        },
                    )
                }
                persist()
                if (stopped.size != targets.size) {
                    serviceSuppressed = false
                    settings.edit().putBoolean("serviceSuppressed", false).apply()
                    _ui.update { it.copy(error = "Some transfers could not be stopped. Please retry.") }
                    syncService(force = true)
                    return@withLock
                }
                PlayService.stop(app)
                onFinished?.invoke()
            }
        }
    }

    fun stopAllAndExit(onFinished: (() -> Unit)? = null) {
        if (isShuttingDown.getAndSet(true)) return
        serviceSuppressed = true
        settings.edit().putBoolean("serviceSuppressed", true).apply()

        debounceJob?.cancel()
        prefetchJob?.cancel()
        storageStatsJob?.cancel()
        eventJob?.cancel()

        scope.launch {
            initialized.await()
            mutex.withLock {
                var failed = false
                val prepId = _ui.value.prepare?.engineId
                commitInProgress = false
                _ui.update { it.copy(prepare = null, addSheetOpen = false) }
                if (prepId != null) {
                    recentlyInvalidatedIds.add(prepId)
                    try {
                        withContext(io) { client.remove(prepId, true) }
                    } catch (t: Exception) {
                        if (t is CancellationException) throw t
                        failed = true
                    }
                }

                val allLive = _ui.value.library.filter { it.engineId != null }
                val genMap = allLive.associate { it.key to (it.generation + 1) }
                _ui.update { state ->
                    state.copy(
                        library = state.library.map { entry ->
                            val nextGen = genMap[entry.key]
                            if (nextGen != null) {
                                entry.copy(
                                    generation = nextGen,
                                    lifecycleState = EntryLifecycleState.STOPPING,
                                )
                            } else entry
                        },
                    )
                }

                // Stop all torrent network activity and seeding by removing active torrents from engine
                // WITHOUT calling client.shutdown() or /shutdown endpoint
                val stopped = mutableSetOf<String>()
                for (entry in allLive) {
                    val id = entry.engineId ?: continue
                    recentlyInvalidatedIds.add(id)
                    try {
                        withContext(io) { client.remove(id, false) }
                        stopped += entry.key
                    } catch (t: Exception) {
                        if (t is CancellationException) throw t
                        failed = true
                        recentlyInvalidatedIds.remove(id)
                        patch(entry.key) { it.copy(lifecycleState = entry.lifecycleState, error = readableError(t)) }
                    }
                }

                _ui.update { state ->
                    state.copy(
                        library = state.library.map { entry ->
                            if (entry.key in stopped) {
                                entry.copy(
                                    engineId = null,
                                    paused = true,
                                    status = null,
                                    lifecycleState = if (entry.complete) EntryLifecycleState.COMPLETED else EntryLifecycleState.STOPPED,
                                )
                            } else entry
                        },
                    )
                }
                persist()
                if (failed) {
                    isShuttingDown.set(false)
                    serviceSuppressed = false
                    settings.edit().putBoolean("serviceSuppressed", false).apply()
                    _ui.update { it.copy(error = "Some transfers could not be stopped. Retry Shutdown.") }
                    eventJob?.cancel()
                    eventJob = scope.launch { eventLoop() }
                    syncService()
                    return@withLock
                }
                PlayService.stop(app)
                shutdownComplete = true
                (app as? WebtorApp)?.currentActivity()?.finish()
                onFinished?.invoke()
            }
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
        val generation = draftGeneration
        var createdId: String? = null
        try {
            val addedId = mutex.withLock {
                if (isShuttingDown.get() || generation != draftGeneration) return
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
                    _ui.update { it.copy(addSheetOpen = true, error = null) }
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
                // Capture the returned ID even when cancellation arrives during blocking HTTP.
                val added = withContext(io + NonCancellable) {
                    client.add(torrentId, prepare = true, torrentData = torrentData).also { createdId = it.id }
                }
                if (generation != draftGeneration || isShuttingDown.get()) throw CancellationException("Draft replaced")
                if (_ui.value.library.any { it.engineId == added.id || it.key.equals(added.infoHash, true) }) {
                    runCatching { releasePreparedEngine(added.id) }
                    createdId = null
                    _ui.update { it.copy(prepare = null, addSheetOpen = true, error = null) }
                    return
                }
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
            if (t is CancellationException) {
                createdId?.let { id ->
                    if (_ui.value.library.none { it.engineId == id }) {
                        withContext(io + NonCancellable) { runCatching { client.remove(id, true) } }
                    }
                }
                throw t
            }
            if (generation != draftGeneration || isShuttingDown.get()) return
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
        val start = android.os.SystemClock.elapsedRealtime()
        var last: TorrentStatus? = null
        var version = -1L
        while (android.os.SystemClock.elapsedRealtime() - start < timeoutMs) {
            if (forPrepare && _ui.value.prepare?.engineId != id) throw CancellationException("prefetch cleared")
            val t = withContext(io) { client.torrent(id) }
            last = t
            if (t.error != null) error(readableError(t.error))
            if (forPrepare) applyPrepareStatus(t)
            if (t.ready) {
                if (forPrepare) {
                    val selected = _ui.value.prepare?.takeIf { it.engineId == id }?.selected
                    if (selected != null) runCatching { withContext(io) { client.select(id, selected) } }
                }
                return t
            }
            val remaining = timeoutMs - (android.os.SystemClock.elapsedRealtime() - start)
            if (remaining <= 0) break
            version = engineHost.awaitChange(version, remaining)
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
            if (shouldSkipStartupRestore(entry)) continue
            // A peerless magnet has no files or selected destinations yet. It is
            // still a valid saved job and must be restored so discovery can keep
            // running after relaunch.
            if (entry.metadataReady && entry.files.none { it.uri != null }) continue
            restoreEntry(entry, startPaused = false)
        }
    }

    private fun failRestore(key: String, commandGeneration: Long, message: String, retryable: Boolean = false) {
        patch(key) {
            if (it.generation != commandGeneration || it.isDeleting) it
            else it.copy(
                engineId = null,
                // Retain RUN intent so a bounded transient retry can recover.
                // The error state remains visible and is never presented as a
                // user pause.
                paused = false,
                status = null,
                lifecycleState = EntryLifecycleState.ERROR,
                error = message,
            )
        }
        if (retryable) scheduleTransientRestoreRetry(key, commandGeneration)
    }

    private fun schedulePendingMaterialization(entry: DownloadEntry, torrent: TorrentStatus) {
        if (!pendingMaterializations.add(entry.key)) return
        scope.launch {
            var created: List<SavedFile>? = null
            var createdGroupUri: String? = null
            try {
                val live = _ui.value.library.find { it.key == entry.key } ?: return@launch
                if (live.engineId != entry.engineId || live.metadataReady || live.isDeleting || live.paused) return@launch
                val selected = if (live.autoSelect) defaultVideoSelection(torrent.files) else live.selected
                if (selected.isEmpty()) throw IllegalStateException("The torrent contains no downloadable files.")
                val metadata = withContext(io) { runCatching { client.metadata(entry.engineId!!) }.getOrDefault("") }
                val skeleton = live.copy(
                    metadata = metadata,
                    metadataReady = true,
                    files = torrent.files.map {
                        SavedFile(
                            it.index, it.name, it.path, it.length,
                            progress = it.progress,
                            verifiedBytes = it.verifiedBytes.coerceIn(0L, it.length.coerceAtLeast(0L)),
                        )
                    },
                    selected = selected,
                )
                val free = withContext(io) { storage.freeBytes }
                val needed = selected.sumOf { index -> skeleton.files.first { it.index == index }.length }
                check(needed <= free) {
                    "Not enough storage. This download needs ${formatBytes(needed)}; ${formatBytes(free)} is available."
                }
                val createdResult = withContext(io) { storage.createFiles(skeleton, storage.folder) }
                created = createdResult.files
                createdGroupUri = createdResult.groupUri
                val pfds = withContext(io) { storage.openFiles(created!!, selected) }
                var accepted = false
                try {
                    val descriptors = descriptorsFor(created!!, pfds, selected)
                    mutex.withLock {
                        val beforeConfigure = _ui.value.library.find { it.key == entry.key }
                        if (beforeConfigure != null && restoreStillApplies(
                                beforeConfigure.generation,
                                entry.generation,
                                beforeConfigure.isDeleting,
                                shutdownInProgress(),
                            )) {
                            // Pause/delete commands take this same mutex. The
                            // generation check and native configure are thus a
                            // single serialized state transition.
                            withContext(io) { client.configure(entry.engineId!!, descriptors, selected) }
                            val latest = _ui.value.library.find { it.key == entry.key }
                            if (latest != null && restoreStillApplies(
                                    latest.generation,
                                    entry.generation,
                                    latest.isDeleting,
                                    shutdownInProgress(),
                                )) {
                                if (latest.paused) withContext(io) { client.pause(entry.engineId!!) }
                                else withContext(io) { client.resume(entry.engineId!!) }
                                patch(entry.key) {
                                    if (it.generation != entry.generation || it.isDeleting) it
                                    else it.copy(
                                        metadata = metadata,
                                        metadataReady = true,
                                        files = created!!,
                                        downloadGroupUri = createdGroupUri,
                                        selected = selected,
                                        error = null,
                                        lifecycleState = if (it.paused) EntryLifecycleState.PAUSED else EntryLifecycleState.DOWNLOADING,
                                    )
                                }
                                accepted = true
                            }
                        }
                    }
                } finally {
                    pfds.forEach { runCatching { it?.close() } }
                }
                if (!accepted) {
                    withContext(io + NonCancellable) { runCatching { client.remove(entry.engineId!!, false) } }
                    withContext(io) { storage.deleteFiles(created!!) }
                    created = null
                    val latest = _ui.value.library.find { it.key == entry.key }
                    if (latest?.let { it.engineId == entry.engineId && it.paused } == true) {
                        patch(entry.key) {
                            if (it.engineId == entry.engineId && it.paused) {
                                it.copy(engineId = null, status = null, lifecycleState = EntryLifecycleState.PAUSED)
                            } else it
                        }
                    }
                    return@launch
                }
                created = null
                persist()
                refreshStorageStats()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                created?.let { runCatching { withContext(io) { storage.deleteFiles(it) } } }
                patch(entry.key) {
                    if (it.engineId != entry.engineId || it.isDeleting) it else it.copy(error = readableError(t))
                }
            } finally {
                pendingMaterializations.remove(entry.key)
            }
        }
    }

    private suspend fun restoreEntry(entry: DownloadEntry, startPaused: Boolean, commandGeneration: Long? = null) {
        val current = _ui.value.library.find { it.key == entry.key } ?: return
        if (current.isDeleting) return
        val expectedGen = commandGeneration ?: current.generation
        if (commandGeneration != null && current.generation != commandGeneration) return
        if (shutdownInProgress()) {
            failRestore(current.key, expectedGen, "Transfers were stopped. Try Resume again.")
            return
        }
        val hasAnyUri = current.files.any { it.uri != null }
        if (hasAnyUri) {
            val hasFiles = current.files.any { it.index in current.selected && it.uri != null }
            // An explicit empty selection is a valid configured/stopped state.
            // It must be restored with payload disabled so a later reselect can
            // attach a destination without losing this torrent's identity.
            if (current.selected.isNotEmpty() && !hasFiles) {
                failRestore(current.key, expectedGen, "This download has no saved files to restore.")
                return
            }
        } else {
            if (current.selected.isEmpty()) {
                failRestore(current.key, expectedGen, "Select at least one file to download.")
                return
            }
            if (Build.VERSION.SDK_INT < 29) {
                failRestore(current.key, expectedGen, "Torrent Player needs Android 10 or newer to save into Downloads.")
                return
            }
            val needed = current.files.filter { it.index in current.selected }.sumOf { it.length }
            val free = withContext(io) { storage.freeBytes }
            if (needed > free) {
                failRestore(
                    current.key,
                    expectedGen,
                    "Not enough storage. This download needs ${formatBytes(needed)}; ${formatBytes(free)} is available.",
                )
                return
            }
        }
        if (current.metadata.isBlank() && current.source.isBlank()) {
            failRestore(current.key, expectedGen, "This download has no torrent metadata to restore.")
            return
        }
        if (!_ui.value.engineReady) waitForEngine()
        if (!_ui.value.engineReady) {
            failRestore(current.key, expectedGen, "The download engine is not running. Close Torrent Player and open it again.")
            return
        }
        var addedId: String? = null
        var createdFiles: List<SavedFile>? = null
        var createdGroupUri: String? = null
        try {
            val torrentId = current.source.ifBlank { current.key }
            val data = current.metadata.takeIf { it.isNotBlank() }
            val added = withContext(io + NonCancellable) {
                client.add(torrentId, prepare = true, torrentData = data).also { addedId = it.id }
            }
            recentlyInvalidatedIds.remove(added.id)

            if (!current.metadataReady && current.files.isEmpty()) {
                // Metadata may require a peer and can take an arbitrary amount
                // of time. Keep the native magnet alive and let the event loop
                // materialize files when the metadata-ready bit changes.
                val latest = _ui.value.library.find { it.key == current.key }
                if (latest == null || !restoreStillApplies(latest.generation, expectedGen, latest.isDeleting, shutdownInProgress())) {
                    withContext(io + NonCancellable) { runCatching { client.remove(added.id, false) } }
                    return
                }
                val status = runCatching { withContext(io) { client.torrent(added.id) } }.getOrNull()
                patch(current.key) {
                    if (it.generation != expectedGen || it.isDeleting) it
                    else it.copy(
                        engineId = added.id,
                        status = status ?: it.status,
                        paused = startPaused,
                        error = null,
                        lifecycleState = if (startPaused) EntryLifecycleState.PAUSED else EntryLifecycleState.DOWNLOADING,
                    )
                }
                if (startPaused) withContext(io) { client.pause(added.id) }
                persist()
                syncService(force = true)
                return
            }
            waitReady(added.id, timeoutMs = 30_000, forPrepare = false)

            val fetchedMetadata = if (current.metadata.isNotBlank()) current.metadata else {
                runCatching { withContext(io) { client.metadata(added.id) } }.getOrDefault("")
            }

            val effectiveFiles = if (!hasAnyUri) {
                val skeleton = current.copy(metadata = fetchedMetadata)
                val createdResult = withContext(io) { storage.createFiles(skeleton, storage.folder) }
                createdFiles = createdResult.files
                createdGroupUri = createdResult.groupUri
                createdResult.files
            } else {
                current.files
            }

            // Opening once both validates access and supplies configuration.
            val pfds = withContext(io) { storage.openFiles(effectiveFiles, current.selected) }
            try {
                val descriptors = descriptorsFor(effectiveFiles, pfds, current.selected)
                withContext(io) { client.configure(added.id, descriptors, current.selected) }
            } catch (e: EngineException) {
                if (e.message?.contains("already configured", ignoreCase = true) != true) throw e
            } finally {
                pfds.forEach { runCatching { it?.close() } }
            }
            if (startPaused) {
                withContext(io) { client.pause(added.id) }
            } else {
                withContext(io) { client.resume(added.id) }
            }
            val latest = _ui.value.library.find { it.key == current.key }
            if (latest == null) {
                createdFiles?.let { runCatching { withContext(io) { storage.deleteFiles(it) } } }
                withContext(io + NonCancellable) { client.remove(added.id, false) }
                return
            }
            if (!restoreStillApplies(latest.generation, expectedGen, latest.isDeleting, shutdownInProgress())) {
                createdFiles?.let { runCatching { withContext(io) { storage.deleteFiles(it) } } }
                withContext(io + NonCancellable) { client.remove(added.id, false) }
                if (latest.isDeleting || latest.generation != expectedGen) return
                failRestore(current.key, expectedGen, "Transfers were stopped. Try Resume again.")
                return
            }
            if (!hasAnyUri) {
                _events.trySend(UiEvent.RequestNotifications)
            }
            val status = runCatching { withContext(io) { client.torrent(added.id) } }.getOrNull()
            patch(current.key) {
                if (it.generation != expectedGen || it.isDeleting) it
                else it.copy(
                    engineId = added.id,
                    files = effectiveFiles,
                    downloadGroupUri = createdGroupUri ?: it.downloadGroupUri,
                    status = status ?: it.status,
                    metadata = if (it.metadata.isNotBlank()) it.metadata else fetchedMetadata,
                    paused = startPaused,
                    error = null,
                    lifecycleState = when {
                        current.selected.isEmpty() -> EntryLifecycleState.STOPPED
                        startPaused -> EntryLifecycleState.PAUSED
                        else -> EntryLifecycleState.DOWNLOADING
                    },
                )
            }
        } catch (t: Throwable) {
            createdFiles?.let { runCatching { withContext(io) { storage.deleteFiles(it) } } }
            addedId?.let { id -> withContext(io + NonCancellable) { runCatching { client.remove(id, false) } } }
            if (t is CancellationException) {
                failRestore(current.key, expectedGen, "Could not resume this download.")
                throw t
            }
            failRestore(current.key, expectedGen, readableError(t), retryable = isTransientRestoreFailure(t))
        }
    }

    private fun isTransientRestoreFailure(error: Throwable): Boolean {
        if (error is CancellationException) return false
        val message = error.message.orEmpty().lowercase()
        return listOf(
            "not enough storage", "folder access", "file is unavailable",
            "no torrent metadata", "select at least one", "android 10",
        ).none { permanent -> permanent in message }
    }

    private fun cancelRetry(key: String) {
        retryJobs.remove(key)?.cancel()
        retryStates.remove(key)
    }

    private fun scheduleTransientRestoreRetry(key: String, generation: Long) {
        val live = _ui.value.library.find { it.key == key } ?: return
        if (live.paused || live.isDeleting || live.generation != generation) return
        val episode = RetryEpisode(key.hashCode().toLong(), generation)
        when (val directive = RetryPolicy.onTransientFailure(retryStates[key], episode, TransferIntent.RUN)) {
            is RetryDirective.Schedule -> scheduleRetry(key, directive)
            RetryDirective.Cancel -> cancelRetry(key)
            else -> Unit
        }
    }

    private fun scheduleRetry(key: String, directive: RetryDirective.Schedule) {
        retryJobs.remove(key)?.cancel()
        retryStates[key] = directive.state
        patch(key) { entry ->
            if (entry.generation == directive.state.episode.generation && !entry.isDeleting) {
                entry.copy(transferStage = TransferStage.RETRYING, transferReason = "Will retry shortly")
            } else entry
        }
        retryJobs[key] = scope.launch {
            delay(directive.afterMillis)
            mutex.withLock {
                val currentState = retryStates[key] ?: return@withLock
                val started = RetryPolicy.markAttemptStarted(currentState, directive.state.episode) ?: return@withLock
                retryStates[key] = started
                val live = _ui.value.library.find { it.key == key } ?: return@withLock
                if (live.paused || live.isDeleting || live.generation != started.episode.generation) {
                    cancelRetry(key)
                    return@withLock
                }
                patch(key) { it.copy(error = null, lifecycleState = EntryLifecycleState.PREPARING) }
                restoreEntry(live, startPaused = false, commandGeneration = live.generation)
                reconcileTransferState()
            }
        }
    }

    private fun advanceRetriesForNetworkRecovery() {
        retryStates.toMap().forEach { (key, state) ->
            val live = _ui.value.library.find { it.key == key } ?: return@forEach
            when (val directive = RetryPolicy.onNetworkRecovered(state, state.episode, if (live.paused) TransferIntent.PAUSED else TransferIntent.RUN)) {
                is RetryDirective.Schedule -> scheduleRetry(key, directive)
                RetryDirective.Cancel -> cancelRetry(key)
                else -> Unit
            }
        }
    }

    private data class StatusTarget(val key: String, val engineId: String, val generation: Long)

    private suspend fun eventLoop() {
        initialized.await()
        var eventVersion = -1L
        while (true) {
            if (isShuttingDown.get()) break
            val nextVersion = try {
                withContext(io) { engineHost.awaitChange(eventVersion) }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                delay(500)
                continue
            }
            if (isShuttingDown.get()) break
            if (nextVersion == eventVersion) continue
            eventVersion = nextVersion
            val state = _ui.value
            if (!state.engineReady) continue

            val targets = state.library.mapNotNull { e ->
                val id = e.engineId ?: return@mapNotNull null
                if (e.isDeleting || e.lifecycleState == EntryLifecycleState.PREPARING || e.lifecycleState == EntryLifecycleState.DELETING || e.lifecycleState == EntryLifecycleState.STOPPING || e.lifecycleState == EntryLifecycleState.PAUSING) {
                    return@mapNotNull null
                }
                StatusTarget(e.key, id, e.generation)
            }
            val prepareTarget = if (prefetchJob?.isActive != true) state.prepare?.engineId else null

            var changed = false
            var completed = false
            for (target in targets) {
                if (isShuttingDown.get()) break
                try {
                    val t = withContext(io) { client.torrent(target.engineId) }
                    val current = _ui.value.library.find { it.key == target.key }
                    if (current != null && current.engineId == target.engineId && current.generation == target.generation && !current.isDeleting) {
                        val beforeDownloaded = current.downloaded
                        applyStatus(t, target.key, target.generation)
                        val after = _ui.value.library.find { it.key == target.key }
                        if (after != null && !after.metadataReady && t.ready && t.files.isNotEmpty()) {
                            schedulePendingMaterialization(after, t)
                        }
                        if (after != null && after.downloaded != beforeDownloaded) changed = true
                        if (after?.complete == true && current.complete != true) completed = true
                    }
                } catch (e: EngineException) {
                    val current = _ui.value.library.find { it.key == target.key }
                    val isIntentional = current == null ||
                        current.engineId != target.engineId ||
                        current.generation != target.generation ||
                        current.isDeleting ||
                        current.lifecycleState == EntryLifecycleState.DELETING ||
                        current.lifecycleState == EntryLifecycleState.STOPPING ||
                        recentlyInvalidatedIds.contains(target.engineId)

                    val is404 = e.statusCode == 404

                    if (is404 && isIntentional) {
                        // Silent expected lifecycle 404 suppression
                    } else if (is404 && !state.restoring && current != null && current.generation == target.generation && !current.isDeleting) {
                        markMissingEngine(target.engineId, e)
                    }
                } catch (_: Throwable) {
                }
            }
            if (prepareTarget != null && !isShuttingDown.get()) {
                try {
                    val t = withContext(io) { client.torrent(prepareTarget) }
                    applyPrepareStatus(t)
                } catch (_: Throwable) {}
            }
            val now = System.currentTimeMillis()
            if ((changed && now - lastPersistAt > 10_000) || completed) {
                persist()
                lastPersistAt = now
            }
            if (state.screen == Screen.Settings) refreshStorageStats()
            if (!serviceSuppressed && !isShuttingDown.get()) {
                syncService()
            }
        }
    }

    private fun applyStatus(t: TorrentStatus, key: String, generation: Long) {
        var completed: DownloadEntry? = null
        _ui.update { state ->
            val idx = state.library.indexOfFirst { it.key == key }
            if (idx < 0) return@update state
            val entry = state.library[idx]
            if (entry.generation != generation || entry.isDeleting) return@update state
            val files = entry.files.map { f ->
                val tf = t.files.find { it.index == f.index }
                // Keep the saved progress visible while rechecking. Only the
                // final verified result may correct it (e.g. after corruption).
                if (tf != null && !t.checking) {
                    f.copy(
                        progress = tf.progress,
                        verifiedBytes = tf.verifiedBytes.coerceIn(0L, f.length.coerceAtLeast(0L)),
                    )
                } else {
                    f
                }
            }
            val lifecycleState = when {
                t.error != null -> EntryLifecycleState.ERROR
                !entry.metadataReady -> if (t.paused) EntryLifecycleState.PAUSED else EntryLifecycleState.DOWNLOADING
                entry.selected.isEmpty() -> EntryLifecycleState.STOPPED
                !t.checking && files.filter { it.index in entry.selected }.all { it.isVerifiedComplete } -> EntryLifecycleState.COMPLETED
                t.paused -> EntryLifecycleState.PAUSED
                else -> EntryLifecycleState.DOWNLOADING
            }
            val next = entry.copy(
                files = files,
                status = t,
                paused = entry.paused,
                lifecycleState = lifecycleState,
                error = t.error?.let { readableError(it) },
                title = t.name?.takeIf { it.isNotBlank() } ?: entry.title,
            )
            if (shouldNotifyCompletion(entry, next)) completed = next.copy(completionAck = next.generation)
            state.copy(library = state.library.toMutableList().also { it[idx] = next })
        }
        completed?.let { done ->
            patch(key) { if (it.generation == done.generation) done else it }
            scope.launch { persist(); PlayService.notifyCompleted(app, done) }
        }
        scope.launch { mutex.withLock { reconcileTransferState() } }
    }

    private fun savedNetworkPolicy(): TransferNetworkPolicy = settings
        .getString("networkPolicy", TransferNetworkPolicy.ANY.name)
        ?.let { name -> TransferNetworkPolicy.entries.find { it.name == name } }
        ?: TransferNetworkPolicy.ANY

    /**
     * Refresh visible transfer reasons from shared policy. Eligible torrents run
     * simultaneously with no slot limits; native selection follows the saved
     * selected intent via restore/configure and manual toggle paths.
     */
    private fun reconcileTransferState() {
        _ui.update { current ->
            current.copy(
                library = current.library.map { entry ->
                    val decision = decideTransfer(entry.asTransferInput(current))
                    entry.copy(
                        transferStage = decision.stage,
                        transferReason = decision.visibleReason,
                    )
                },
            )
        }
    }

    private fun DownloadEntry.asTransferInput(state: UiState) = TransferDecisionInput(
        intent = when {
            isDeleting -> TransferIntent.DELETED
            paused -> TransferIntent.PAUSED
            else -> TransferIntent.RUN
        },
        engineReady = state.engineReady,
        metadataReady = metadataReady,
        hasSelection = selected.isNotEmpty(),
        storage = storageReadiness(this),
        networkAllowed = state.networkAllowed,
        verificationActive = checking,
    )

    private fun storageReadiness(entry: DownloadEntry): StorageReadiness = when {
        entry.error?.contains("folder access", ignoreCase = true) == true ||
            entry.error?.contains("file is unavailable", ignoreCase = true) == true -> StorageReadiness.ACCESS_REQUIRED
        entry.error?.contains("not enough storage", ignoreCase = true) == true -> StorageReadiness.FATAL_ERROR
        else -> StorageReadiness.READY
    }

    private fun markMissingEngine(id: String, error: EngineException) {
        if (recentlyInvalidatedIds.contains(id)) return
        val msg = readableError(error)
        _ui.update { state ->
            state.copy(
                library = state.library.map { entry ->
                    if (entry.engineId != id || entry.isDeleting) entry
                    else entry.copy(
                        engineId = null,
                        paused = true,
                        status = null,
                        lifecycleState = if (entry.complete) EntryLifecycleState.COMPLETED else EntryLifecycleState.ERROR,
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
            // A descriptor represents a durable Android destination, not just
            // current download demand. Preserve descriptors for deselected
            // files so a later reselect can reuse their saved bytes.
            fds[file.index] = pfds.getOrNull(i)?.fd
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
        persistenceMutex.withLock {
            val snapshot = _ui.value.library
            try {
                withContext(io) { storage.save(snapshot) }
            } catch (t: Exception) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(loadWarning = "Could not save download changes. Free some storage before closing the app.") }
            }
        }
    }

    private fun syncService(force: Boolean = false) {
        if (serviceSuppressed || isShuttingDown.get()) {
            PlayService.stop(app)
            lastServiceUpdate = 0L
            return
        }
        val live = _ui.value.library.filter {
            it.engineId != null && !it.complete && !it.isDeleting && it.lifecycleState != EntryLifecycleState.DELETING
        }
        if (live.isEmpty()) {
            PlayService.stop(app)
            lastServiceUpdate = 0L
            return
        }
        val downloading = live.filter { !it.paused }
        if (downloading.isEmpty()) {
            PlayService.stop(app)
            lastServiceUpdate = 0L
            return
        }
        val total = live.sumOf { it.total }
        val got = live.sumOf { it.downloaded }
        val progress = if (total <= 0L) 0 else ((got.toDouble() / total) * 100).toInt().coerceIn(0, 100)
        val title = if (live.size == 1) live.first().title else "${live.size} downloads"
        val text = if (downloading.all { it.checking }) {
            "Checking saved data · ${downloading.map { it.checkPercent }.average().toInt()}%"
        } else {
            "${formatBytes(got)} / ${formatBytes(total)}  ·  ${formatSpeed(downloading.sumOf { it.status?.downloadSpeed ?: 0L })}"
        }
        val now = android.os.SystemClock.elapsedRealtime()
        if (force || now - lastServiceUpdate >= 1000) {
            PlayService.start(
                app,
                title,
                progress,
                paused = false,
                text = text,
                multiple = live.size > 1,
            )
            lastServiceUpdate = now
        }
    }

    private suspend fun waitForEngine() {
        val ready = withContext(io) { engineHost.awaitReady() }
        if (ready && runCatching { withContext(io) { client.stats() } }.isSuccess) {
            _ui.update { it.copy(engineReady = true, statusLine = "Ready") }
            return
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

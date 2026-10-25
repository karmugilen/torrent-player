package webtor.app

import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
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
    private var lastStatsRefresh = 0L
    private var draftGeneration = 0L
    private var commitInProgress = false
    private val retryStates = mutableMapOf<String, RetryState>()
    private val retryJobs = mutableMapOf<String, Job>()
    private var networkSnapshot = NetworkSnapshot()
    private val networkUpdates = Channel<Unit>(Channel.CONFLATED)
    @Volatile private var shutdownComplete = false
    private val antiDpiMutex = Mutex()
    private var pendingAntiDpiTarget: Boolean? = null
    private var batteryReceiverRegistered = false
    private var batteryPercent: Int = -1
    private var batteryCharging = false
    private var playNextLatchedUri: String? = null
    private var activeFileOnlyPlayback: Pair<String, String>? = null

    private val _ui = MutableStateFlow(UiState(networkPolicy = savedNetworkPolicy(), networkAllowed = false))
    val ui: StateFlow<UiState> = _ui.asStateFlow()

    private val _events = Channel<UiEvent>(Channel.BUFFERED)
    val events = _events.receiveAsFlow()

    fun onForeground() {
        reopenAfterShutdown()
        scope.launchCommand {
            runCatching { refreshAutoRamCache() }.onFailure {
                if (it is CancellationException) throw it
                Log.w(EngineHost.TAG, "Could not refresh automatic RAM cache", it)
            }
        }
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
        registerBatteryMonitor()
        scope.launch { applyNetworkUpdates() }
        serviceSuppressed = settings.getBoolean("serviceSuppressed", false)
        // Reset previous custom connection tuning once; subsequent user changes persist.
        if (!settings.getBoolean("peerDefaultsV2", false)) {
            settings.edit().putInt("maxPeers", 55).putBoolean("peerDefaultsV2", true).apply()
        }
        refreshStorageStats()
        scope.launch {
            try {
                val loaded = withContext(io) { storage.load() }.map {
                    val staysPaused = it.paused || serviceSuppressed || it.directPlay
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
                // Unsaved settings follow UiState defaults, so a new install has one source of truth.
                val defaults = UiState()
                val pauseOnLowBattery = settings.getBoolean("pauseOnLowBattery", defaults.pauseOnLowBattery)
                _ui.update {
                    it.copy(
                        library = loaded,
                        loadWarning = storage.loadWarning,
                        maxPeers = settings.getInt("maxPeers", 55).coerceIn(8, 500),
                        ramCacheMiB = settings.getInt("ramCacheMiB", 64).coerceIn(RAM_CACHE_STEPS_MIB.first(), RAM_CACHE_STEPS_MIB.last()),
                        ramCacheAuto = settings.getBoolean("ramCacheAuto", !settings.contains("ramCacheMiB")),
                        seedingEnabled = settings.getBoolean("seedingEnabled", true),
                        seedRatioMilli = settings.getInt("seedRatioMilli", 1000).coerceIn(100, 10000),
                        seedTimeMinutes = settings.getInt("seedTimeMinutes", 60).coerceIn(1, 1440),
                        seedUploadKiB = settings.getInt("seedUploadKiB", 256).coerceIn(16, 4096),
                        seedWiFiOnly = settings.getBoolean("seedWiFiOnly", true),
                        seedChargingOnly = settings.getBoolean("seedChargingOnly", false),
                        restoring = loaded.isNotEmpty(),
                        statusLine = if (loaded.isEmpty()) it.statusLine else "Restoring downloads…",
                        darkTheme = settings.getBoolean("darkTheme", true),
                        appTheme = parseSavedTheme(settings.getString("appTheme", "AERO")),
                        dynamicColors = settings.getBoolean("dynamicColors", true),
                        reducedEffects = settings.getBoolean("reducedEffects", false),
                        directPlayDefault = settings.getBoolean("directPlayDefault", false),
                        playbackMethod = PlaybackMethod.entries.firstOrNull {
                            it.name == settings.getString("playbackMethod", PlaybackMethod.HTTP.name)
                        } ?: PlaybackMethod.HTTP,
                        antiDpiEnabled = settings.getBoolean("antiDpiEnabled", false),
                        networkPolicy = savedNetworkPolicy(),
                        idleDropMinutes = settings.getInt("idleDropMinutes", defaults.idleDropMinutes).coerceIn(0, 240),
                        fileOnlyPlayback = settings.getBoolean("fileOnlyPlayback", defaults.fileOnlyPlayback),
                        hedgeRequests = settings.getBoolean("hedgeRequests", defaults.hedgeRequests),
                        slowPeerYield = settings.getBoolean("slowPeerYield", defaults.slowPeerYield),
                        dropChokers = settings.getBoolean("dropChokers", defaults.dropChokers),
                        pauseOnLowBattery = pauseOnLowBattery,
                        batteryPaused = pauseOnLowBattery && batteryPercent in 0..15 && !batteryCharging,
                        playNext = settings.getBoolean("playNext", defaults.playNext),
                        downloadRateKiB = savedDownloadRateKiB(),
                    )
                }
                if (_ui.value.batteryPaused) networkUpdates.trySend(Unit)
                // Publish the local library before slow metadata and descriptor
                // recovery so unrelated commands remain responsive.
                initialized.complete(Unit)
                settings.edit().also { editor ->
                    editor.remove("playerPackage").remove("playerActivity")
                    editor.remove("downloadRateBytesSec").remove("uploadRateBytesSec")
                    // Playback has one built-in mode; forget the old experiment switches.
                    RETIRED_EXPERIMENT_PREF_KEYS.forEach(editor::remove)
                    LEGACY_DOWNLOAD_QUEUE_PREF_KEYS.forEach(editor::remove)
                }.apply()
                refreshStorageStats()
                waitForEngine()
                if (_ui.value.engineReady) {
                    runCatching { withContext(io) { client.setMaxPeers(_ui.value.maxPeers) } }
                    runCatching { withContext(io) { client.setDownloadRateBytesSec(_ui.value.downloadRateKiB.toLong() * 1024L) } }
                    runCatching {
                        mutex.withLock {
                            withContext(io) { client.setDialRatePerSec(if (_ui.value.fastPeerSearch) 80 else 30) }
                            withContext(io) { client.setPeerExploration(_ui.value.peerExploration) }
                            withContext(io) { client.setGhostMode(_ui.value.ghostMode) }
                            withContext(io) { client.setGhostProMode(_ui.value.ghostProMode) }
                            withContext(io) { client.setClaudeSeekMode(_ui.value.claudeSeekMode) }
                            withContext(io) { client.setHedgeRequests(_ui.value.hedgeRequests) }
                            withContext(io) { client.setSlowPeerYield(_ui.value.slowPeerYield) }
                            withContext(io) { client.setDropChokers(_ui.value.dropChokers) }
                            withContext(io) { client.setIdleDropMinutes(_ui.value.idleDropMinutes) }
                        }
                    }
                    runCatching {
                        if (_ui.value.ramCacheAuto) refreshAutoRamCache()
                        else withContext(io) { client.setRamCacheMiB(_ui.value.ramCacheMiB) }
                    }
                        .onFailure { failure ->
                            if (failure is CancellationException) throw failure
                            _ui.update { it.copy(error = readableError(failure)) }
                        }
                    applySeedingSettings(force = true)
                    runCatching { withContext(io) { client.setPayloadTransfersAllowed(payloadAllowed()) } }
                    setAntiDpi(_ui.value.antiDpiEnabled)
                    val nativeCache = runCatching { withContext(io) { client.getCache() } }.getOrNull()
                    if (nativeCache != null) {
                        _ui.update { it.copy(nativeCacheSummary = nativeCache) }
                    }
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
    suspend fun awaitInitialization() { initialized.await() }

    fun openSettings() {
        refreshStorageStats()
        _ui.update { it.copy(screen = Screen.Settings, error = null) }
    }

    fun setDarkTheme(dark: Boolean) {
        settings.edit().putBoolean("darkTheme", dark).apply()
        _ui.update { it.copy(darkTheme = dark) }
    }

    fun setDynamicColors(enabled: Boolean) {
        settings.edit().putBoolean("dynamicColors", enabled).apply()
        _ui.update { it.copy(dynamicColors = enabled) }
    }

    fun setAppTheme(theme: AppTheme) {
        settings.edit().putString("appTheme", theme.name).apply()
        _ui.update { it.copy(appTheme = theme) }
    }

    fun setReducedEffects(enabled: Boolean) {
        settings.edit().putBoolean("reducedEffects", enabled).apply()
        _ui.update { it.copy(reducedEffects = enabled) }
    }

    fun setAntiDpi(enabled: Boolean) {
        settings.edit().putBoolean("antiDpiEnabled", enabled).apply()
        _ui.update { it.copy(antiDpiEnabled = enabled) }
        pendingAntiDpiTarget = enabled
        if (_ui.value.engineReady) {
            scope.launch {
                antiDpiMutex.withLock {
                    while (true) {
                        val target = pendingAntiDpiTarget ?: break
                        pendingAntiDpiTarget = null
                        try {
                            val status = withContext(io) { client.setBypass(target) }
                            if (pendingAntiDpiTarget == null) {
                                _ui.update { it.copy(bypassStatus = status, error = null) }
                            }
                        } catch (t: Throwable) {
                            if (t is CancellationException) throw t
                            if (pendingAntiDpiTarget == null) {
                                _ui.update { it.copy(error = readableError(t)) }
                            }
                        }
                    }
                }
            }
        }
    }

    fun clearNativeCache(categories: List<String>) {
        if (!_ui.value.engineReady) return
        scope.launchCommand {
            try {
                val before = withContext(io) { client.getCache() }
                val after = withContext(io) { client.clearCache(categories) }
                val reclaimed = (before.categories.sumOf { it.bytes } - after.categories.sumOf { it.bytes }).coerceAtLeast(0L)
                _ui.update { it.copy(nativeCacheSummary = after,
                    message = "${formatBytes(reclaimed)} cleared. Active playback data is retained.", error = null) }
            } catch (t: Exception) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t)) }
            }
        }
    }
    fun setNetworkPolicy(policy: TransferNetworkPolicy) {
        settings.edit().putString("networkPolicy", policy.name).apply()
        updateNetworkState(networkSnapshot, policy)
    }

    fun setIdleDropMinutes(minutes: Int) {
        val value = minutes.coerceIn(0, 240)
        scope.launchCommand {
            try {
                if (_ui.value.engineReady) withContext(io) { client.setIdleDropMinutes(value) }
                settings.edit().putInt("idleDropMinutes", value).apply()
                _ui.update { it.copy(idleDropMinutes = value) }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t)) }
            }
        }
    }

    /** Engine ids already idle-dropped, so repeated status events drop once. Ids are not reused after removal. */
    private val pendingIdleDrops = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()

    /** Ends a Direct Play session: peers disconnect and RAM is freed. Play starts it again. */
    fun drop(entry: DownloadEntry, idle: Boolean = false) {
        if (!entry.directPlay) return
        if (idle) {
            val id = entry.engineId ?: return
            if (!pendingIdleDrops.add(id)) return
        }
        stop(entry)
        if (idle) _ui.update { it.copy(message = "Stopped idle Direct Play: ${entry.title}") }
    }

    fun refreshSeekStats() {
        if (!_ui.value.engineReady) return
        scope.launch {
            val stats = runCatching { withContext(io) { client.seekStats() } }.getOrNull() ?: return@launch
            _ui.update { it.copy(seekStats = stats) }
        }
    }

    fun setHedgeRequests(enabled: Boolean) {
        settings.edit().putBoolean("hedgeRequests", enabled).apply()
        _ui.update { it.copy(hedgeRequests = enabled) }
        scope.launch {
            if (_ui.value.engineReady) runCatching { withContext(io) { client.setHedgeRequests(enabled) } }
            refreshSeekStats()
        }
    }

    fun setSlowPeerYield(enabled: Boolean) {
        settings.edit().putBoolean("slowPeerYield", enabled).apply()
        _ui.update { it.copy(slowPeerYield = enabled) }
        scope.launch {
            if (_ui.value.engineReady) runCatching { withContext(io) { client.setSlowPeerYield(enabled) } }
            refreshSeekStats()
        }
    }


    fun setDropChokers(enabled: Boolean) {
        settings.edit().putBoolean("dropChokers", enabled).apply()
        _ui.update { it.copy(dropChokers = enabled) }
        scope.launch {
            if (_ui.value.engineReady) runCatching { withContext(io) { client.setDropChokers(enabled) } }
            refreshSeekStats()
        }
    }

    fun setFileOnlyPlayback(enabled: Boolean) {
        settings.edit().putBoolean("fileOnlyPlayback", enabled).apply()
        _ui.update { it.copy(fileOnlyPlayback = enabled) }
        if (!enabled) finishPlayback(null)
    }

    /** A player return or failed launch releases the saved torrent's file filter. */
    fun finishPlayback(uri: String?) {
        scope.launchCommand {
            if (uri != null && activeFileOnlyPlayback?.first != uri) return@launchCommand
            try {
                clearFileOnlyPlayback()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t)) }
            }
        }
    }

    private suspend fun clearFileOnlyPlayback() {
        val active = activeFileOnlyPlayback ?: return
        if (_ui.value.library.any { it.engineId == active.second }) {
            withContext(io) { client.clearFileOnly(active.second) }
        }
        activeFileOnlyPlayback = null
    }

    fun setPauseOnLowBattery(enabled: Boolean) {
        settings.edit().putBoolean("pauseOnLowBattery", enabled).apply()
        _ui.update { it.copy(pauseOnLowBattery = enabled) }
        publishBatteryPaused()
        networkUpdates.trySend(Unit)
    }

    fun setPlayNext(enabled: Boolean) {
        settings.edit().putBoolean("playNext", enabled).apply()
        _ui.update { it.copy(playNext = enabled) }
    }

    fun setDownloadRateKiB(value: Int) {
        if (value != 0 && value != 1024 && value != 4096 && value != 8192) return
        settings.edit().putInt("downloadRateKiB", value).apply()
        _ui.update { it.copy(downloadRateKiB = value) }
        if (_ui.value.engineReady) {
            val bytes = value.toLong() * 1024L
            scope.launch {
                runCatching { withContext(io) { client.setDownloadRateBytesSec(bytes) } }
            }
        }
    }

    fun updateNetworkState(snapshot: NetworkSnapshot) =
        updateNetworkState(snapshot, _ui.value.networkPolicy)

    private fun updateNetworkState(snapshot: NetworkSnapshot, policy: TransferNetworkPolicy) {
        val wasAllowed = _ui.value.networkAllowed
        networkSnapshot = snapshot
        val allowed = transferAllowed(policy, snapshot.connected, snapshot.wifi, snapshot.metered)
        _ui.update { it.copy(networkPolicy = policy, networkAllowed = allowed, networkReason = if (allowed) null else "Waiting for ${policy.name.lowercase().replace('_', ' ')} network") }
        networkUpdates.trySend(Unit)
        scope.launch {
            mutex.withLock {
                reconcileTransferState()
                if (!wasAllowed && _ui.value.networkAllowed) advanceRetriesForNetworkRecovery()
            }
        }
    }

    private fun payloadAllowed(): Boolean = _ui.value.networkAllowed && !_ui.value.batteryPaused

    private fun registerBatteryMonitor() {
        if (batteryReceiverRegistered) return
        batteryReceiverRegistered = true
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) {
                if (intent?.action != Intent.ACTION_BATTERY_CHANGED) return
                onBatteryChanged(intent)
            }
        }
        val filter = IntentFilter(Intent.ACTION_BATTERY_CHANGED)
        val sticky = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            app.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            app.registerReceiver(receiver, filter)
        }
        if (sticky != null) onBatteryChanged(sticky)
    }

    private fun onBatteryChanged(intent: Intent) {
        val level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
        if (level >= 0 && scale > 0) batteryPercent = level * 100 / scale
        val status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
        batteryCharging = status == BatteryManager.BATTERY_STATUS_CHARGING ||
            status == BatteryManager.BATTERY_STATUS_FULL
        if (publishBatteryPaused()) networkUpdates.trySend(Unit)
    }

    private fun publishBatteryPaused(): Boolean {
        val paused = _ui.value.pauseOnLowBattery && batteryPercent in 0..15 && !batteryCharging
        if (paused == _ui.value.batteryPaused) return false
        _ui.update { it.copy(batteryPaused = paused) }
        return true
    }

    private suspend fun applyNetworkUpdates() {
        for (ignored in networkUpdates) {
            // One worker serializes JNI updates. Bursts collapse to the latest
            // route, including events received while the engine was starting.
            while (!engineHost.awaitReady()) delay(1_000)
            while (networkUpdates.tryReceive().isSuccess) { /* drain superseded events */ }
            val snapshot = networkSnapshot
            val allowed = payloadAllowed()
            try {
                withContext(io) {
                    client.setPayloadTransfersAllowed(allowed)
                    client.networkChanged(snapshot.generation, snapshot.connected)
                }
                applySeedingSettings()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                Log.w(EngineHost.TAG, "Network recovery request failed; retrying", failure)
                delay(2_000)
                networkUpdates.trySend(Unit)
            }
        }
    }

    fun pickFolder() { _events.trySend(UiEvent.PickFolder) }

    fun persistFolder(uri: android.net.Uri) {
        runCatching {
            val displayName = android.provider.DocumentsContract.getTreeDocumentId(uri)
                .substringAfterLast(':').trim().ifBlank { "Selected folder" }
            storage.persistFolder(uri, displayName)
        }.onSuccess {
            refreshStorageStats()
            _ui.update { it.copy(message = "Download folder updated") }
        }.onFailure {
            showError("Folder access is unavailable")
        }
    }

    fun clearFolder() {
        storage.clearFolder()
        refreshStorageStats()
        _ui.update { it.copy(message = "Reverted to default Downloads/Webtor folder") }
    }

    fun requestRepairFile(key: String, fileIndex: Int) {
        _events.trySend(UiEvent.PickFileForRepair(key, fileIndex))
    }

    fun repairFileUri(key: String, fileIndex: Int, newUri: android.net.Uri) {
        cancelRetry(key)
        scope.launchCommand {
            val entry = _ui.value.library.find { it.key == key } ?: return@launchCommand
            if (entry.isDeleting || entry.directPlay) return@launchCommand
            val file = entry.files.find { it.index == fileIndex } ?: return@launchCommand
            try {
                withContext(io) {
                    app.contentResolver.takePersistableUriPermission(newUri,
                        android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
                    val size = app.contentResolver.openFileDescriptor(newUri, "rw")?.use { it.statSize }
                        ?: error("Cannot access this file. Choose a writable document.")
                    check(size < 0L || size == file.length) { "Choose the original file (${formatBytes(file.length)}). Selected size: ${formatBytes(size)}." }
                    entry.engineId?.let { client.remove(it, false) }
                }
                entry.engineId?.let { recentlyInvalidatedIds.add(it) }
                patch(key) { current -> current.copy(
                    generation = current.generation + 1L,
                    engineId = null, status = null, paused = true,
                    lifecycleState = EntryLifecycleState.STOPPED,
                    files = current.files.map { saved ->
                        if (saved.index == fileIndex) saved.copy(uri = newUri.toString(), progress = 0.0, verifiedBytes = 0L) else saved
                    }, error = null,
                ) }
                withContext(io) { (app as? WebtorApp)?.thumbnails?.invalidateEntryKey(key) }
                persist()
                syncService(force = true)
                _ui.update { it.copy(message = "File linked. Resume to verify the saved data before downloading.") }
            } catch (t: Exception) {
                if (t is CancellationException) throw t
                _ui.update { it.copy(error = readableError(t)) }
            }
        }
    }

    fun closeSettings() = _ui.update { it.copy(screen = Screen.Library, clearAllConfirm = false) }
    fun setMaxPeers(value: Int) {
        val n = value.coerceIn(8, 500)
        settings.edit().putInt("maxPeers", n).apply()
        _ui.update { it.copy(maxPeers = n) }
    }

    fun commitMaxPeers() {
        val n = _ui.value.maxPeers
        scope.launch {
            if (_ui.value.engineReady) runCatching { withContext(io) { client.setMaxPeers(n) } }
        }
    }

    fun setRamCacheMiB(value: Int) {
        val size = value.coerceIn(RAM_CACHE_STEPS_MIB.first(), RAM_CACHE_STEPS_MIB.last())
        scope.launchCommand {
            try {
                val applied = if (_ui.value.engineReady) {
                    withContext(io) { client.setRamCacheMiB(size) }
                } else size
                settings.edit().putInt("ramCacheMiB", applied).putBoolean("ramCacheAuto", false).apply()
                _ui.update { it.copy(ramCacheMiB = applied, ramCacheAuto = false) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                _ui.update { it.copy(error = readableError(failure)) }
            }
        }
    }

    fun setAutoRamCache() {
        scope.launchCommand {
            try {
                val applied = applyAutoRamCache()
                settings.edit().putBoolean("ramCacheAuto", true).apply()
                _ui.update { it.copy(ramCacheMiB = applied, ramCacheAuto = true) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                _ui.update { it.copy(error = readableError(failure)) }
            }
        }
    }

    private suspend fun refreshAutoRamCache() {
        if (!_ui.value.ramCacheAuto || !_ui.value.engineReady) return
        val applied = applyAutoRamCache()
        _ui.update { it.copy(ramCacheMiB = applied) }
    }

    private suspend fun applyAutoRamCache(): Int {
        val state = _ui.value
        val active = state.library.count {
            !it.paused && !it.isDeleting && (!it.complete || it.liveStatus()?.seeding == true)
        }.coerceAtLeast(1)
        val size = withContext(io) {
            val manager = app.getSystemService(android.app.ActivityManager::class.java)
            val memory = android.app.ActivityManager.MemoryInfo()
            manager.getMemoryInfo(memory)
            autoRamCacheMiB(memory.totalMem, memory.availMem, memory.threshold, manager.isLowRamDevice || memory.lowMemory, active)
        }
        if (!state.engineReady) return size
        return try {
            withContext(io) { client.setRamCacheMiB(size) }
        } catch (failure: EngineException) {
            // An active Direct Play torrent may need larger buffers to verify
            // its pieces. Retain the working limit until that session closes.
            if (size < state.ramCacheMiB && failure.message?.contains("active Direct Play torrent") == true) state.ramCacheMiB
            else throw failure
        }
    }
    private var lastSeedSignature: String? = null

    private suspend fun applySeedingSettings(force: Boolean = false) {
        if (!_ui.value.engineReady) return
        val state = _ui.value
        val battery = app.registerReceiver(null, android.content.IntentFilter(android.content.Intent.ACTION_BATTERY_CHANGED))
        val batteryStatus = battery?.getIntExtra(android.os.BatteryManager.EXTRA_STATUS, -1)
        val charging = batteryStatus == android.os.BatteryManager.BATTERY_STATUS_CHARGING || batteryStatus == android.os.BatteryManager.BATTERY_STATUS_FULL
        val playing = (app as? WebtorApp)?.playbackLeases?.activeKeys().orEmpty().isNotEmpty()
        val allowed = state.networkAllowed && networkSnapshot.connected && (!state.seedWiFiOnly || networkSnapshot.wifi) && (!state.seedChargingOnly || charging) && !playing
        val signature = "${state.seedingEnabled}:$allowed:${state.seedRatioMilli}:${state.seedTimeMinutes}:${state.seedUploadKiB}"
        if (!force && signature == lastSeedSignature) return
        runCatching { withContext(io) { client.setSeeding(state.seedingEnabled, allowed, state.seedRatioMilli, state.seedTimeMinutes, state.seedUploadKiB) } }
            .onSuccess { lastSeedSignature = signature }
            .onFailure { if (it is CancellationException) throw it; _ui.update { s -> s.copy(error = readableError(it)) } }
    }

    fun setSeedingEnabled(value: Boolean) {
        settings.edit().putBoolean("seedingEnabled", value).apply()
        _ui.update { it.copy(seedingEnabled = value) }
        scope.launchCommand {
            applySeedingSettings()
            if (value) for (entry in _ui.value.library.toList()) {
                if (entry.complete && !entry.directPlay && !entry.paused && !entry.isDeleting && entry.engineId == null) restoreEntry(entry, startPaused = false)
            }
            syncService(force = true)
        }
    }
    fun setSeedRatio(value: Int) { settings.edit().putInt("seedRatioMilli", value).apply(); _ui.update { it.copy(seedRatioMilli = value) }; scope.launch { applySeedingSettings() } }
    fun setSeedTime(value: Int) { settings.edit().putInt("seedTimeMinutes", value).apply(); _ui.update { it.copy(seedTimeMinutes = value) }; scope.launch { applySeedingSettings() } }
    fun setSeedUpload(value: Int) { settings.edit().putInt("seedUploadKiB", value).apply(); _ui.update { it.copy(seedUploadKiB = value) }; scope.launch { applySeedingSettings() } }
    fun setSeedWiFi(value: Boolean) { settings.edit().putBoolean("seedWiFiOnly", value).apply(); _ui.update { it.copy(seedWiFiOnly = value) }; scope.launch { applySeedingSettings() } }
    fun setSeedCharging(value: Boolean) { settings.edit().putBoolean("seedChargingOnly", value).apply(); _ui.update { it.copy(seedChargingOnly = value) }; scope.launch { applySeedingSettings() } }
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

    fun selectVideoFiles() {
        _ui.update { state ->
            val files = state.prepare?.torrent?.files ?: return@update state
            state.copy(prepare = state.prepare.copy(selected = defaultVideoSelection(files), error = null))
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
                val effectiveFiles = if (!current.directPlay && selecting && file.uri.isNullOrBlank()) {
                    // Allocate the destination only when the user requests the
                    // file. The operation is reversible if /select fails.
                    withContext(io) { storage.createAdditionalFile(current, file) }.also { addedFile = it }
                    current.files.map { if (it.index == index) addedFile!! else it }
                } else current.files
                val id = current.engineId
                if (id != null) {
                    val pfds = if (current.directPlay) emptyList() else withContext(io) { storage.openFiles(effectiveFiles, next) }
                    try {
                        val descriptors = if (current.directPlay) null else descriptorsFor(effectiveFiles, pfds, next)
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

    fun setDirectPlayDefault(enabled: Boolean) {
        settings.edit().putBoolean("directPlayDefault", enabled).apply()
        _ui.update { it.copy(directPlayDefault = enabled) }
    }

    fun setPlaybackMethod(method: PlaybackMethod) {
        settings.edit().putString("playbackMethod", method.name).apply()
        _ui.update { it.copy(playbackMethod = method) }
    }

    private fun directPlayMemoryBytes(): Long {
        return _ui.value.ramCacheMiB.toLong() * 1024 * 1024
    }

    fun startDownload() = commitPreparedDownload()
    fun startDirectPlay() = commitPreparedDownload(directPlay = true)

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

    private fun commitPreparedDownload(directPlay: Boolean = false) {
        scope.launch {
            mutex.withLock {
                if (commitInProgress || !beginUserWork()) return@withLock
                val draft = _ui.value.prepare ?: return@withLock
                val torrent = draft.torrent
                if (torrent?.ready != true) {
                    if (directPlay) { setPrepareError("Waiting for file information. You can save the magnet as a download meanwhile."); return@withLock }
                    val source = draft.source.trim()
                    val key = infoHashFromMagnet(source) ?: draft.engineId
                    if (_ui.value.library.any { it.key.equals(key, true) || it.engineId == draft.engineId }) {
                        setPrepareError("This torrent is already in your library.")
                        return@withLock
                    }
                    val pending = DownloadEntry(
                        key = key,
                        generation = withContext(io) { storage.nextGeneration(key) },
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
                if (!directPlay && Build.VERSION.SDK_INT < 29) {
                    setPrepareError("Torrent Player needs Android 10 or newer to save into Downloads.")
                    return@withLock
                }
                commitInProgress = true
                val key = torrent.infoHash ?: draft.engineId
                val provisional = DownloadEntry(
                    directPlay = directPlay,
                    key = key,
                    generation = withContext(io) { storage.nextGeneration(key) },
                    title = torrent.name ?: key,
                    source = torrent.magnetURI ?: draft.source,
                    metadata = "",
                    engineId = draft.engineId,
                    destination = if (directPlay) "Direct Play · RAM + startup cache" else "Downloads/Webtor",
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
                    check(directPlay || needed <= free) {
                        "Not enough storage. This download needs ${formatBytes(needed)}; ${formatBytes(free)} is available."
                    }

                    val metadata = withContext(io) { client.metadata(draft.engineId) }
                    val skeleton = provisional.copy(metadata = metadata)
                    val createdResult = if (directPlay) CreatedDownloadFiles(skeleton.files) else withContext(io) { storage.createFiles(skeleton, storage.folder) }
                    val created = createdResult.files
                    try {
                        val pfds = if (directPlay) emptyList() else withContext(io) { storage.openFiles(created, draft.selected) }
                        try {
                            val descriptors = if (directPlay) List<Int?>(created.size) { null } else descriptorsFor(created, pfds, draft.selected)
                            refreshAutoRamCache()
                            withContext(io) { client.configure(draft.engineId, descriptors, draft.selected, if (directPlay) "stream" else "download", directPlayMemoryBytes()) }
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
                    if (directPlay) play(entry)
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

    fun pauseBatch(keys: Set<String>) {
        keys.forEach { key ->
            _ui.value.library.find { it.key == key }?.let { pause(it) }
        }
    }

    fun resumeBatch(keys: Set<String>) {
        keys.forEach { key ->
            _ui.value.library.find { it.key == key }?.let { resume(it) }
        }
    }

    fun deleteBatch(keys: Set<String>, eraseFiles: Boolean) {
        if (isShuttingDown.get()) return
        val targets = _ui.value.library.filter { it.key in keys }
        if (targets.isEmpty()) return
        targets.forEach { cancelRetry(it.key) }
        val nextGens = targets.associate { it.key to (it.generation + 1) }

        _ui.update { state ->
            state.copy(
                library = state.library.map { entry ->
                    val nextGen = nextGens[entry.key]
                    if (nextGen != null) {
                        entry.copy(
                            isDeleting = true,
                            lifecycleState = EntryLifecycleState.DELETING,
                            generation = nextGen,
                        )
                    } else entry
                }
            )
        }
        syncService()

        scope.launchCommand {
            var failureCount = 0
            targets.forEach { target ->
                val current = _ui.value.library.find { it.key == target.key } ?: return@forEach
                val targetGen = nextGens[target.key] ?: (target.generation + 1)
                if (current.generation != targetGen) return@forEach

                var engineRemoved = current.engineId == null
                val id = current.engineId
                try {
                    if (id != null) {
                        recentlyInvalidatedIds.add(id)
                        withContext(io) { client.remove(id, destroyStore = eraseFiles) }
                        engineRemoved = true
                    }
                    if (eraseFiles) {
                        withContext(io) {
                            storage.deleteFiles(current.files)
                            if (current.directPlay) storage.deleteStartupCache(current.key)
                        }
                    }
                    withContext(io) { (app as? WebtorApp)?.thumbnails?.invalidate(current) }
                    withContext(io) { storage.retireTorrent(current.key, targetGen) }
                    _ui.update { state ->
                        val latest = state.library.find { it.key == current.key }
                        if (latest?.generation == targetGen) {
                            state.copy(
                                library = state.library.filter { it.key != current.key },
                                screen = if (state.openTorrentKey == current.key) Screen.Library else state.screen,
                                openTorrentKey = state.openTorrentKey.takeUnless { it == current.key },
                            )
                        } else state
                    }
                } catch (t: Throwable) {
                    if (t is CancellationException) throw t
                    failureCount++
                    if (!engineRemoved && id != null) {
                        recentlyInvalidatedIds.remove(id)
                    }
                    patch(current.key) { e ->
                        if (e.generation == targetGen) {
                            e.copy(
                                isDeleting = false,
                                lifecycleState = EntryLifecycleState.ERROR,
                                engineId = if (engineRemoved) null else e.engineId,
                                paused = if (engineRemoved) true else e.paused,
                                status = if (engineRemoved) null else e.status,
                                error = readableError(t),
                            )
                        } else e
                    }
                }
            }
            persist()
            refreshStorageStats()
            syncService()
            if (failureCount > 0) {
                _ui.update { it.copy(message = "Could not remove $failureCount item(s)") }
            }
        }
    }

    fun resume(entry: DownloadEntry) {
        scope.launchCommand {
            if (!beginUserWork()) return@launchCommand

            val current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
            if (current.isDeleting || current.complete) return@launchCommand
            if (requiresFileSelectionForResume(current)) {
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

    fun recordPlaybackPosition(uri: String, positionMs: Long, durationMs: Long) {
        if (positionMs < 0) return
        scope.launchCommand {
            val entry = _ui.value.library.find { e -> e.files.any { PlaybackProvider.uriFor(e, it).toString() == uri } } ?: return@launchCommand
            val ended = entry.files.find { PlaybackProvider.uriFor(entry, it).toString() == uri }
            patch(entry.key) { e -> e.copy(files = e.files.map { f ->
                if (PlaybackProvider.uriFor(e, f).toString() != uri) f else f.withPlaybackPosition(positionMs, durationMs)
            }) }
            persist()
            maybePlayNext(uri, entry.key, ended, positionMs, durationMs)
        }
    }

    private fun maybePlayNext(uri: String, entryKey: String, ended: SavedFile?, positionMs: Long, durationMs: Long) {
        val atEnd = durationMs > 0L && positionMs >= durationMs - 3_000L
        if (!atEnd) {
            if (playNextLatchedUri == uri) playNextLatchedUri = null
            return
        }
        if (!_ui.value.playNext || ended == null || playNextLatchedUri == uri) return
        if (!ended.name.isVideoName() && !ended.path.isVideoName()) return
        val current = _ui.value.library.find { it.key == entryKey } ?: return
        val next = nextVideoAfter(current, ended) ?: return
        playNextLatchedUri = uri
        play(current, next.index)
    }

    fun play(entry: DownloadEntry, fileIndex: Int? = null, fromStart: Boolean = false) {
        scope.launchCommand {
            if (!beginUserWork()) return@launchCommand
            try {
                var current = _ui.value.library.find { it.key == entry.key } ?: return@launchCommand
                if (current.isDeleting) return@launchCommand
                val targetIndex = fileIndex ?: pickPlayIndex(current)
                check(targetIndex != null && (targetIndex in current.selected || completedPlayFile(current, targetIndex) != null)) {
                    "This file was not selected for download."
                }
                if (fromStart) {
                    patch(current.key) { e -> e.copy(files = e.files.map { if (it.index == targetIndex) it.copy(playbackPositionMs = 0) else it }) }
                    current = _ui.value.library.find { it.key == entry.key } ?: current
                }
                clearFileOnlyPlayback()
                completedPlayFile(current, targetIndex)?.let { file ->
                    withContext(io) { storage.prepareForPlayback(file) }
                    val content = playbackEvent(current, file)
                    val id = current.engineId
                    if (_ui.value.fileOnlyPlayback && !current.directPlay && !current.paused &&
                        targetIndex in current.selected && current.status?.checking != true && id != null) {
                        withContext(io) { client.play(id, targetIndex, fileOnly = true) }
                        activeFileOnlyPlayback = content.uri to id
                    }
                    _events.send(content)
                    if (fromStart) persist()
                    return@launchCommand
                }
                if (!current.directPlay && current.files.none { it.index in current.selected && it.uri != null }) {
                    error("Start this download before playing.")
                }
                // Refuse before restore. Starting a dropped session and then
                // showing the battery or network message left that session
                // downloading on its own once the block lifted.
                playbackDataBlock(_ui.value.batteryPaused, _ui.value.networkAllowed, _ui.value.networkReason)?.let { error(it) }
                if (current.engineId == null) {
                    val nextGen = current.generation + 1
                    patch(current.key) {
                        it.copy(generation = nextGen, lifecycleState = EntryLifecycleState.PREPARING, error = null)
                    }
                    restoreEntry(current, startPaused = false, commandGeneration = nextGen)
                    current = _ui.value.library.find { it.key == entry.key } ?: current
                }
                val id = current.engineId ?: error(current.error ?: "Could not start this torrent.")
                // The engine refuses unfinished files while torrent data is paused.
                playbackDataBlock(_ui.value.batteryPaused, _ui.value.networkAllowed, _ui.value.networkReason)?.let { error(it) }
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
                val file = current.files.first { it.index == targetIndex }
                val content = playbackEvent(current, file)
                val fileOnly = _ui.value.fileOnlyPlayback && !current.directPlay
                val info = withContext(io) {
                    client.play(
                        id, targetIndex, if (fromStart) 0L else file.resumeByteHint(),
                        boost = _ui.value.playBoost, fileOnly = fileOnly, adaptiveBuffer = _ui.value.adaptiveBuffer,
                        burstAwareBuffer = _ui.value.burstAwareBuffer,
                        nextFileIndex = nextEpisodeToWarm(current, file, _ui.value.playNext)?.index,
                    )
                }
                if (fileOnly) activeFileOnlyPlayback = content.uri to id
                if (_ui.value.preloadStart) awaitPlaybackPreload(id, targetIndex, file.length)
                _events.send(if (_ui.value.playbackMethod == PlaybackMethod.HTTP) httpPlaybackEvent(content, current, file, info.streamUrl) else content)
                persist()
                syncService()
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                runCatching { clearFileOnlyPlayback() }
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
        warmDirectPlay(entry.key)
    }

    fun openDetails(key: String) {
        closeAddSheet()
        _ui.update { it.copy(screen = Screen.Files, openTorrentKey = key, error = null) }
        warmDirectPlay(key)
    }

    /**
     * Starts a dropped Direct Play session while the episode list is open, so
     * peers are already connected when an episode is played.
     */
    private fun warmDirectPlay(key: String) {
        fun eligible(entry: DownloadEntry?) = entry != null && entry.directPlay && entry.engineId == null &&
            !entry.isDeleting && !serviceSuppressed && !shutdownInProgress() && payloadAllowed()
        if (!eligible(_ui.value.library.find { it.key == key })) return
        scope.launchCommand {
            val current = _ui.value.library.find { it.key == key }
            if (!eligible(current)) return@launchCommand
            val nextGen = current!!.generation + 1
            patch(current.key) { it.copy(generation = nextGen, lifecycleState = EntryLifecycleState.PREPARING, error = null) }
            restoreEntry(current, startPaused = false, commandGeneration = nextGen)
            persist()
            syncService()
        }
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
            var engineRemoved = target.engineId == null
            try {
                val id = target.engineId
                if (id != null) {
                    recentlyInvalidatedIds.add(id)
                    // Stream cache has nothing useful to keep in Downloads; always wipe it.
                    withContext(io) { client.remove(id, false) }
                    engineRemoved = true
                }
                // Drop keeps peer addresses for the next Play. Delete forgets
                // them even when that drop already cleared the engine id.
                torrentInfoHash(target)?.let { hash ->
                    withContext(io) { runCatching { client.forgetPeerHints(hash) } }
                }
                withContext(io) { (app as? WebtorApp)?.thumbnails?.invalidate(target) }
                _ui.update { state ->
                    state.copy(library = state.library.filter { it.key != target.key },
                        screen = if (state.openTorrentKey == target.key) Screen.Library else state.screen,
                        openTorrentKey = state.openTorrentKey.takeUnless { it == target.key })
                }
                withContext(io) { storage.retireTorrent(target.key, nextGen) }
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
                torrentInfoHash(target)?.let { hash ->
                    withContext(io) { runCatching { client.forgetPeerHints(hash) } }
                }
                withContext(io) { storage.deleteFiles(target.files); if (target.directPlay) storage.deleteStartupCache(target.key) }
                withContext(io) { (app as? WebtorApp)?.thumbnails?.invalidate(target) }
                _ui.update { state ->
                    state.copy(library = state.library.filter { it.key != target.key },
                        screen = if (state.openTorrentKey == target.key) Screen.Library else state.screen,
                        openTorrentKey = state.openTorrentKey.takeUnless { it == target.key })
                }
                withContext(io) { storage.retireTorrent(target.key, nextGen) }
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
                if (entry.directPlay) return@map entry
                try {
                    entry.engineId?.let { id ->
                        recentlyInvalidatedIds.add(id)
                        withContext(io) { client.remove(id, false) }
                    }
                    patch(entry.key) { it.copy(engineId = null, paused = true, status = null) }
                    withContext(io) { storage.deleteFiles(entry.files) }
                    withContext(io) { (app as? WebtorApp)?.thumbnails?.invalidate(entry) }
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
        scope.launchCommand {
            try {
                var reclaimedNative = 0L
                withContext(io) {
                    storage.clearCache()
                    (app as? WebtorApp)?.thumbnails?.clearDiskCache()
                    if (_ui.value.engineReady) {
                        val before = client.getCache().categories.sumOf { it.bytes }
                        val after = client.clearCache(listOf("startup", "resume", "peers", "dht"))
                        reclaimedNative = (before - after.categories.sumOf { it.bytes }).coerceAtLeast(0L)
                    }
                }
                refreshStorageStats()
                if (_ui.value.engineReady) {
                    val summary = withContext(io) { client.getCache() }
                    _ui.update { it.copy(nativeCacheSummary = summary) }
                }
                _ui.update {
                    val msg = if (reclaimedNative > 0L) {
                        "Cache cleared: ${formatBytes(reclaimedNative)} reclaimed."
                    } else {
                        "Temporary files were removed."
                    }
                    it.copy(message = msg, error = null)
                }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
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
                it.engineId != null && !it.paused && (!it.complete || it.liveStatus()?.seeding == true) && !it.isDeleting
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
        (app as? WebtorApp)?.httpPlaybackLease?.clear()
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
        (app as? WebtorApp)?.httpPlaybackLease?.clear()
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
            val free = withContext(io) { storage.freeBytes }
            val cache = withContext(io) { storage.cacheBytes }
            val legacy = withContext(io) { storage.legacyBytes }
            val folderName = withContext(io) { if (storage.folder == null) "Downloads/Webtor" else storage.folderName }
            val hasCustom = withContext(io) { storage.folder != null }
            val warning = withContext(io) { storage.storageAccessWarning() }
            _ui.update {
                it.copy(
                    freeBytes = free,
                    managedBytes = storage.managedBytes(it.library),
                    cacheBytes = cache,
                    legacyBytes = legacy,
                    storageFolderName = folderName,
                    hasCustomFolder = hasCustom,
                    storageAccessWarning = warning,
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
            // Commands are already enabled while startup runs. Re-read intent
            // under the same lock so pause/resume cannot race an old snapshot.
            mutex.withLock {
                val live = _ui.value.library.find { it.key == entry.key } ?: return@withLock
                if (shouldRestoreAtStartup(live) || (_ui.value.seedingEnabled && live.complete && !live.directPlay && !live.paused && !live.isDeleting && live.engineId == null)) restoreEntry(live, startPaused = false)
            }
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
        val validationError = restoreValidationError(current)
        if (validationError != null) {
            failRestore(current.key, expectedGen, validationError)
            return
        }
        val hasAnyUri = current.files.any { it.uri != null }
        if (!current.directPlay && !hasAnyUri && (current.metadataReady || current.files.isNotEmpty())) {
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
        if (!_ui.value.engineReady) waitForEngine()
        if (!_ui.value.engineReady) {
            failRestore(current.key, expectedGen, "The download engine is not running. Close Torrent Player and open it again.")
            return
        }
        var addedId: String? = null
        var createdFiles: List<SavedFile>? = null
        var createdGroupUri: String? = null
        try {
            val added = withContext(io + NonCancellable) {
                client.addSavedDownload(current).also { addedId = it.id }
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
                // /add may return an existing paused record during Retry.
                withContext(io) {
                    if (startPaused) client.pause(added.id) else client.resume(added.id)
                }
                val status = runCatching { withContext(io) { client.torrent(added.id) } }.getOrNull()
                patch(current.key) {
                    if (it.generation != expectedGen || it.isDeleting) it
                    else restoredPendingDiscovery(it, added.id, status, startPaused)
                }
                val restored = _ui.value.library.find { it.key == current.key }
                if (!startPaused && restored?.engineId == added.id && status?.ready == true && status.files.isNotEmpty()) {
                    schedulePendingMaterialization(restored, status)
                }
                persist()
                syncService(force = true)
                return
            }
            waitReady(added.id, timeoutMs = 30_000, forPrepare = false)

            val fetchedMetadata = if (current.metadata.isNotBlank()) current.metadata else {
                runCatching { withContext(io) { client.metadata(added.id) } }.getOrDefault("")
            }

            val effectiveFiles = if (!current.directPlay && !hasAnyUri) {
                val skeleton = current.copy(metadata = fetchedMetadata)
                val createdResult = withContext(io) { storage.createFiles(skeleton, storage.folder) }
                createdFiles = createdResult.files
                createdGroupUri = createdResult.groupUri
                createdResult.files
            } else {
                current.files
            }

            // Opening once both validates access and supplies configuration.
            val pfds = if (current.directPlay) emptyList() else withContext(io) { storage.openFiles(effectiveFiles, current.selected) }
            try {
                val descriptors = if (current.directPlay) List<Int?>(effectiveFiles.size) { null } else descriptorsFor(effectiveFiles, pfds, current.selected)
                refreshAutoRamCache()
                withContext(io) { client.configure(added.id, descriptors, current.selected, if (current.directPlay) "stream" else "download", directPlayMemoryBytes()) }
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
            if (!current.directPlay && !hasAnyUri) {
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
                // Bound recovery reads for newly attached/missing status and
                // metadata-pending records; ordinary idle rows stay event-driven.
                withContext(io) { engineHost.awaitChange(eventVersion, 5_000) }
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                delay(500)
                continue
            }
            if (isShuttingDown.get()) break
            val engineChanged = nextVersion != eventVersion
            eventVersion = nextVersion
            val state = _ui.value
            if (!state.engineReady) continue
            if (state.seedingEnabled || lastSeedSignature == null) applySeedingSettings()

            val targets = state.library.mapNotNull { e ->
                val id = e.engineId ?: return@mapNotNull null
                if (!engineChanged && !needsDiscoveryStatusRefresh(e)) return@mapNotNull null
                if (e.isDeleting || e.lifecycleState == EntryLifecycleState.PREPARING || e.lifecycleState == EntryLifecycleState.DELETING || e.lifecycleState == EntryLifecycleState.STOPPING || e.lifecycleState == EntryLifecycleState.PAUSING) {
                    return@mapNotNull null
                }
                StatusTarget(e.key, id, e.generation)
            }
            val prepareTarget = if (engineChanged && prefetchJob?.isActive != true) state.prepare?.engineId else null

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
                        if (after != null && shouldDropIdleDirectPlay(after, t, _ui.value.idleDropMinutes)) drop(after, idle = true)
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
                !entry.directPlay && !t.checking && files.filter { it.index in entry.selected }.all { it.isVerifiedComplete } -> EntryLifecycleState.COMPLETED
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

    private fun savedDownloadRateKiB(): Int {
        val value = settings.getInt("downloadRateKiB", 0)
        return if (value == 0 || value == 1024 || value == 4096 || value == 8192) value else 0
    }

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
        val notification = downloadNotification(_ui.value.library, (app as? WebtorApp)?.playbackLeases?.activeKeys().orEmpty())
        if (notification == null) {
            PlayService.stop(app)
            lastServiceUpdate = 0L
            return
        }
        val now = android.os.SystemClock.elapsedRealtime()
        if (now - lastStatsRefresh >= 2000) {
            // The notification shows them while a video plays elsewhere.
            lastStatsRefresh = now
            refreshSeekStats()
        }
        if (force || now - lastServiceUpdate >= 1000) {
            PlayService.start(
                app,
                notification.title,
                notification.progress,
                paused = false,
                text = notification.text,
                multiple = notification.multiple,
                playbackOnly = notification.playbackOnly,
                stats = playbackStatsLine(_ui.value),
            )
            lastServiceUpdate = now
        }
    }

    private suspend fun awaitPlaybackPreload(id: String, fileIndex: Int, length: Long) {
        val deadline = android.os.SystemClock.elapsedRealtime() + 8_000L
        val needed = minOf(2L * 1024 * 1024, length.coerceAtLeast(0L))
        while (android.os.SystemClock.elapsedRealtime() < deadline) {
            val snapshot = withContext(io) { client.torrent(id) }
            val target = snapshot.files.find { it.index == fileIndex }
            if (target != null && (target.verifiedBytes >= needed || target.progress >= 1.0)) return
            val remaining = deadline - android.os.SystemClock.elapsedRealtime()
            if (remaining <= 0L) return
            delay(minOf(400L, remaining))
        }
    }

    private suspend fun waitForEngine() {
        val ready = withContext(io) { engineHost.awaitReady() }
        val stats = if (ready) runCatching { withContext(io) { client.stats() } }.getOrNull() else null
        if (stats != null) {
            _ui.update { it.copy(engineReady = true, statusLine = "Ready", listenPort = stats.listenPort) }
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

internal fun parseSavedTheme(name: String?): AppTheme = when (name?.uppercase()) {
    "ANTIQUITY" -> AppTheme.ANTIQUITY
    "RETRO" -> AppTheme.RETRO
    "WADA" -> AppTheme.WADA
    else -> AppTheme.AERO
}

/** The video Play next opens after [ended], in natural episode order. */
internal fun nextVideoAfter(entry: DownloadEntry, ended: SavedFile): SavedFile? {
    val order = compareBy<SavedFile> { naturalMediaKey(it.path) }.thenBy { it.index }
    val next = entry.files
        .filter { file ->
            (file.name.isVideoName() || file.path.isVideoName()) &&
                file.index != ended.index &&
                (file.index in entry.selected || file.isVerifiedComplete) &&
                order.compare(ended, file) < 0
        }
        .minWithOrNull(order)
        ?: return null
    if (next.playbackDurationMs > 0L && next.playbackPositionMs >= next.playbackDurationMs - 3_000L) return null
    return next
}

/** The episode to warm near the end of [playing] when Play next is on; null when nothing needs fetching. */
internal fun nextEpisodeToWarm(entry: DownloadEntry, playing: SavedFile, playNext: Boolean): SavedFile? {
    if (!playNext || !(playing.name.isVideoName() || playing.path.isVideoName())) return null
    return nextVideoAfter(entry, playing)?.takeIf { it.index in entry.selected && !it.isVerifiedComplete }
}

/** The engine signals readiness only after a rest with no player open; the app still checks its own state. */
internal fun shouldDropIdleDirectPlay(entry: DownloadEntry, status: TorrentStatus, idleMinutes: Int): Boolean =
    idleMinutes > 0 && entry.directPlay && status.idleDropReady && status.playbackReaders == 0 &&
        entry.engineId == status.id && !entry.paused && !entry.controlsBusy()

package webtor.app

import android.os.SystemClock
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import webtor.core.PieceTelemetry

enum class DetailFileFilter { ALL, VIDEOS, SUBTITLES }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TorrentFilesScreen(
    entry: DownloadEntry,
    error: String?,
    onBack: () -> Unit,
    onPlay: (Int) -> Unit,
    onToggleFile: (Int) -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onDelete: () -> Unit,
    onFocus: (Int) -> Unit = {},
    onClearFocus: () -> Unit = {},
    onStartOver: (Int) -> Unit = {},
    onLocateFile: (Int) -> Unit = {},
) {
    BackHandler(onBack = onBack)
    val colors = MaterialTheme.colorScheme
    val context = LocalContext.current
    val clipboardManager = LocalClipboardManager.current
    val app = context.applicationContext as WebtorApp
    val client = app.engineClient
    val thumbnails = app.thumbnails
    var telemetry by remember(entry.key) { mutableStateOf<PieceTelemetry?>(null) }
    var copiedMagnet by remember(entry.key) { mutableStateOf(false) }
    val magnetUri = resolveMagnetUri(entry)
    LaunchedEffect(copiedMagnet) {
        if (copiedMagnet) {
            delay(2_000)
            copiedMagnet = false
        }
    }
    val previewVideo = firstPreviewVideo(entry)
    val latestEntry by rememberUpdatedState(entry)
    var frames by remember(entry.key) { mutableStateOf(thumbnails.cached(entry)) }
    LaunchedEffect(entry.key, previewVideo?.index, entry.complete, entry.paused, entry.engineId) {
        if (previewVideo == null) return@LaunchedEffect
        while (true) {
            val current = latestEntry
            val next = thumbnails.previews(current)
            if (next.isNotEmpty()) frames = next
            else if (frames == null) frames = emptyList()
            if (current.complete) break
            val live = !current.paused && current.engineId != null
            if (!live) break
            delay(4_000)
        }
    }
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(entry.key, entry.engineId, lifecycleOwner) {
        val engineId = entry.engineId
        // Engine id changes (or going offline) must not keep a stale map.
        telemetry = null
        if (engineId == null) return@LaunchedEffect
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            var eventVersion = -1L
            var lastFetchElapsed = 0L
            // First map loads promptly; later reads stay ≤ 1 Hz.
            val first = withContext(Dispatchers.IO) {
                runCatching { client.pieces(engineId, 256) }.getOrNull()
            }
            if (first != null) telemetry = first
            lastFetchElapsed = SystemClock.elapsedRealtime()
            if (latestEntry.complete) return@repeatOnLifecycle
            while (true) {
                val nextVersion = withContext(Dispatchers.IO) {
                    app.engine.awaitChange(eventVersion)
                }
                if (nextVersion == eventVersion) continue
                eventVersion = nextVersion
                val elapsed = SystemClock.elapsedRealtime() - lastFetchElapsed
                if (elapsed < 1_000L) delay(1_000L - elapsed)
                val next = withContext(Dispatchers.IO) {
                    runCatching { client.pieces(engineId, 256) }.getOrNull()
                }
                if (next != null) telemetry = next
                lastFetchElapsed = SystemClock.elapsedRealtime()
                if (latestEntry.complete) break
            }
        }
    }
    val files = entry.files
    val status = entry.status
    val busy = entry.controlsBusy()

    var fileQuery by rememberSaveable(entry.key) { mutableStateOf("") }
    var naturalOrder by rememberSaveable(entry.key) { mutableStateOf(true) }
    var fileFilter by rememberSaveable(entry.key) { mutableStateOf(DetailFileFilter.ALL) }

    val subtitleMap = remember(entry.files) {
        val videos = entry.files.filter { it.name.isVideoName() || it.path.isVideoName() }
        val videoStems = videos.map { it.path.substringBeforeLast('.').lowercase(Locale.ROOT) }
        val subs = entry.files.filter { it.name.isSubtitleName() || it.path.isSubtitleName() }
        videoStems.associateWith { stem ->
            subs.filter { sub ->
                val p = sub.path.lowercase(Locale.ROOT)
                p == stem || p.startsWith("$stem.") || p.startsWith("$stem/")
            }.map { it.name }
        }
    }

    val displayedFiles = remember(entry.files, fileQuery, naturalOrder, fileFilter) {
        var list = entry.files
        when (fileFilter) {
            DetailFileFilter.VIDEOS -> list = list.filter { it.name.isVideoName() || it.path.isVideoName() }
            DetailFileFilter.SUBTITLES -> list = list.filter { it.name.isSubtitleName() || it.path.isSubtitleName() }
            DetailFileFilter.ALL -> Unit
        }
        if (fileQuery.isNotBlank()) {
            val q = fileQuery.trim()
            list = list.filter { it.name.contains(q, ignoreCase = true) || it.path.contains(q, ignoreCase = true) }
        }
        if (naturalOrder) {
            list = list.sortedWith(compareBy<SavedFile> { naturalMediaKey(it.name) }.thenBy { it.index })
        }
        list
    }

    Scaffold(
        containerColor = Color.Transparent,
        topBar = {
            TopAppBar(
                title = { Text("Download details", style = MaterialTheme.typography.titleLarge) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    if (!busy && !entry.complete) {
                        if (entry.paused || (entry.engineId == null && entry.lifecycleState != EntryLifecycleState.DOWNLOADING)) {
                            TextButton(onClick = onResume) {
                                Text("Resume")
                            }
                        } else {
                            TextButton(onClick = onPause) {
                                Text("Pause")
                            }
                        }
                    }
                    IconButton(onClick = onDelete, enabled = !busy) {
                        Icon(
                            Icons.Default.Delete,
                            contentDescription = "Remove download",
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = colors.surface.copy(alpha = 0.90f),
                    titleContentColor = colors.onBackground,
                    navigationIconContentColor = colors.onBackground,
                    actionIconContentColor = colors.onBackground,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            state = rememberLazyListState(),
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            item {
                DownloadSummary(
                    entry, error, frames,
                    magnetAvailable = magnetUri != null,
                    copiedMagnet = copiedMagnet,
                    onShareMagnet = { magnetUri?.let { shareMagnetUri(context, it) } },
                    onCopyMagnet = {
                        magnetUri?.let {
                            clipboardManager.setText(AnnotatedString(it))
                            copiedMagnet = true
                            Toast.makeText(context, "Magnet link copied", Toast.LENGTH_SHORT).show()
                        }
                    },
                )
            }
            // UI-03: Playable files and episode playback precede diagnostics
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    SectionTitle("Files", "${entry.selected.size} of ${files.size} selected")
                    OutlinedTextField(
                        value = fileQuery,
                        onValueChange = { fileQuery = it },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        placeholder = { Text("Search files & episodes", style = MaterialTheme.typography.bodyMedium) },
                        leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
                        trailingIcon = if (fileQuery.isNotEmpty()) {
                            {
                                IconButton(onClick = { fileQuery = "" }) {
                                    Icon(Icons.Default.Clear, contentDescription = "Clear search")
                                }
                            }
                        } else null,
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        FilterChip(
                            selected = fileFilter == DetailFileFilter.ALL,
                            onClick = { fileFilter = DetailFileFilter.ALL },
                            label = { Text("All") },
                        )
                        FilterChip(
                            selected = fileFilter == DetailFileFilter.VIDEOS,
                            onClick = { fileFilter = DetailFileFilter.VIDEOS },
                            label = { Text("Videos") },
                        )
                        FilterChip(
                            selected = fileFilter == DetailFileFilter.SUBTITLES,
                            onClick = { fileFilter = DetailFileFilter.SUBTITLES },
                            label = { Text("Subtitles") },
                        )
                        Spacer(Modifier.width(8.dp))
                        FilterChip(
                            selected = naturalOrder,
                            onClick = { naturalOrder = !naturalOrder },
                            label = { Text(if (naturalOrder) "Episode order" else "Torrent order") },
                        )
                    }
                }
            }
            if (displayedFiles.isEmpty()) {
                item {
                    Text(
                        "No files match your search or filter.",
                        modifier = Modifier.padding(vertical = 12.dp),
                        style = MaterialTheme.typography.bodyMedium,
                        color = colors.onSurfaceVariant,
                    )
                }
            }
            items(displayedFiles, key = { it.index }) { file ->
                val stem = file.path.substringBeforeLast('.').lowercase(Locale.ROOT)
                TorrentFileRow(
                    file = file,
                    isSelected = file.index in entry.selected,
                    paused = entry.paused,
                    canPlay = !busy && (!entry.checking || completedPlayFile(entry, file.index) != null),
                    directPlay = entry.directPlay,
                    onPlay = { onPlay(file.index) },
                    onStartOver = { onStartOver(file.index) },
                    onToggle = { onToggleFile(file.index) },
                    focused = entry.focusedFile == file.index,
                    onFocus = { onFocus(file.index) },
                    onClearFocus = onClearFocus,
                    canToggle = !busy && !entry.checking && entry.metadataReady,
                    pairedSubtitles = subtitleMap[stem].orEmpty(),
                    onLocate = { onLocateFile(file.index) },
                )
            }
            // Diagnostics follow files
            item { TransferSection(entry) }
            item { ConnectionMapSection(entry, client) }
            item { TrackerDetailsSection(status) }
            item {
                PieceHeatGridSection(
                    map = telemetry,
                    frozen = entry.paused || entry.complete,
                    files = entry.files,
                )
            }
            item { TechnicalSection(entry, status, telemetry, entry.selected.size) }
        }
    }
}

@Composable
private fun DownloadSummary(
    entry: DownloadEntry,
    error: String?,
    frames: List<android.graphics.Bitmap>?,
    magnetAvailable: Boolean,
    copiedMagnet: Boolean,
    onShareMagnet: () -> Unit,
    onCopyMagnet: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val isPaused = entry.paused || entry.lifecycleState == EntryLifecycleState.PAUSED ||
        (entry.metadataReady && (entry.lifecycleState == EntryLifecycleState.STOPPED || entry.engineId == null))
    val (labelBackground, labelColor) = when {
        entry.controlsBusy() || isPaused -> colors.tertiaryContainer to colors.onTertiaryContainer
        error != null || entry.error != null -> colors.errorContainer to colors.onErrorContainer
        else -> colors.primaryContainer to colors.onPrimaryContainer
    }
    val label = downloadSummaryLabel(entry, error)
    val video = entry.files.any { it.index in entry.selected && it.name.isVideoName() }
    val percent = (entry.progress.coerceIn(0f, 1f) * 100).toInt()
    ThemedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (video) VideoPreviewPager(entry, frames, Modifier.fillMaxWidth().aspectRatio(16f / 9f), large = false)
            Text(
                entry.title,
                style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.Bold),
                color = colors.onSurface,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                label,
                Modifier
                    .background(labelBackground, RoundedCornerShape(6.dp))
                    .padding(horizontal = 6.dp, vertical = 2.dp),
                style = MaterialTheme.typography.labelSmall.copy(fontWeight = FontWeight.SemiBold),
                color = labelColor,
            )
            Text(
                if (entry.checking) "Checking saved data · ${entry.checkPercent}%"
                else if (!entry.metadataReady) "Fetching file list…"
                else if (entry.directPlay) "${formatBytes(entry.downloaded)} cached · video stays in RAM"
                else "${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)} · $percent%",
                style = MaterialTheme.typography.bodySmall,
                color = colors.onSurfaceVariant,
            )
            transferTelemetry(entry)?.let { text ->
                Text(text, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
            }
            if (!entry.complete && !entry.directPlay) {
                LinearProgressIndicator(
                    progress = { entry.progress.coerceIn(0f, 1f) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(3.dp)
                        .clip(MaterialTheme.shapes.extraSmall),
                    color = colors.primary,
                    trackColor = colors.surface,
                )
            }
            error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = colors.error) }
            entry.status?.warning?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = colors.tertiary) }
            Row(
                Modifier.fillMaxWidth().padding(top = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                FilledTonalButton(
                    onClick = onShareMagnet,
                    enabled = magnetAvailable,
                    modifier = Modifier.weight(1f).heightIn(min = 48.dp).themedControl(),
                    shape = RoundedCornerShape(12.dp),
                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
                ) {
                    Icon(Icons.Default.Share, contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Share magnet")
                }
                OutlinedButton(
                    onClick = onCopyMagnet,
                    enabled = magnetAvailable,
                    modifier = Modifier.heightIn(min = 48.dp).themedControl(),
                    shape = RoundedCornerShape(12.dp),
                    contentPadding = PaddingValues(horizontal = 16.dp, vertical = 10.dp),
                ) {
                    Text(if (copiedMagnet) "Copied!" else "Copy link")
                }
            }
            if (!magnetAvailable) {
                Text("Magnet link not available yet", style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
            }
        }
    }
}

@Composable
private fun TransferSection(entry: DownloadEntry) {
    val colors = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        SectionTitle("Transfer")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            TransferCard(
                label = "Download",
                value = entry.liveDownloadSpeed()?.let(::formatSpeed) ?: "—",
                modifier = Modifier.weight(1f),
            )
            TransferCard(
                label = "Upload",
                value = entry.liveUploadSpeed()?.let(::formatSpeed) ?: "—",
                modifier = Modifier.weight(1f),
            )
            TransferCard(
                label = "ETA",
                value = formatEta(entry.selectedEta()) ?: "—",
                modifier = Modifier.weight(1f),
            )
        }
        Text(
            entry.liveStatus()?.let { formatPeers(it.numPeers) } ?: "Peers —",
            style = MaterialTheme.typography.labelSmall,
            color = colors.onSurfaceVariant,
            modifier = Modifier.padding(start = 2.dp),
        )
    }
}

@Composable
private fun TransferCard(label: String, value: String, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    Card(
        modifier = modifier,
        shape = RoundedCornerShape(10.dp),
        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
    ) {
        Column(Modifier.padding(horizontal = 8.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                label,
                style = MaterialTheme.typography.labelSmall,
                color = colors.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                value,
                style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold),
                color = colors.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun TrackerDetailsSection(status: webtor.core.TorrentStatus?) {
    val colors = MaterialTheme.colorScheme
    val trackers = status?.trackers.orEmpty()
    var expanded by remember(status?.id) { mutableStateOf(false) }
    val working = trackers.count {
        it.status.equals("working", true) || it.status.equals("connected", true) || it.status.equals("ok", true)
    }
    Card(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
    ) {
        Column {
            TextButton(
                onClick = { expanded = !expanded },
                modifier = Modifier.fillMaxWidth(),
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp),
            ) {
                Text(
                    when {
                        trackers.isEmpty() -> "Connection details · unavailable"
                        else -> "Connection details · $working/${trackers.size} trackers responding"
                    },
                    modifier = Modifier.weight(1f),
                    textAlign = androidx.compose.ui.text.style.TextAlign.Start,
                    color = colors.onSurface,
                )
                Text(if (expanded) "Hide" else "Show", color = colors.primary)
            }
            if (expanded) {
                if (trackers.isEmpty()) {
                    Text(
                        "Tracker status is not available yet.",
                        modifier = Modifier.padding(start = 14.dp, end = 14.dp, bottom = 12.dp),
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.onSurfaceVariant,
                    )
                } else {
                    Column(
                        modifier = Modifier.padding(start = 14.dp, end = 14.dp, bottom = 10.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        trackers.take(MAX_TRACKERS_SHOWN).forEach { tracker ->
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    redactedTrackerUrl(tracker.url),
                                    modifier = Modifier.weight(1f),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurface,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                Text(
                                    tracker.status,
                                    style = MaterialTheme.typography.labelSmall,
                                    color = if (tracker.status.equals("error", true)) colors.error else colors.onSurfaceVariant,
                                )
                            }
                        }
                        if (trackers.size > MAX_TRACKERS_SHOWN) {
                            Text(
                                "+${trackers.size - MAX_TRACKERS_SHOWN} more",
                                style = MaterialTheme.typography.labelSmall,
                                color = colors.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}

private const val MAX_TRACKERS_SHOWN = 12

@Composable
private fun SectionTitle(title: String, subtitle: String? = null) {
    val colors = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(
            title,
            style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
            color = colors.onBackground,
            modifier = Modifier.weight(1f),
        )
        subtitle?.let {
            Text(
                it,
                style = MaterialTheme.typography.labelSmall,
                color = colors.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun TorrentFileRow(
    file: SavedFile,
    isSelected: Boolean,
    paused: Boolean,
    canPlay: Boolean,
    onPlay: () -> Unit,
    onToggle: () -> Unit,
    canToggle: Boolean,
    focused: Boolean = false,
    onFocus: () -> Unit = {},
    onClearFocus: () -> Unit = {},
    directPlay: Boolean = false,
    onStartOver: () -> Unit = {},
    pairedSubtitles: List<String> = emptyList(),
    onLocate: () -> Unit = {},
) {
    val colors = MaterialTheme.colorScheme
    val isVideo = file.name.isVideoName() || file.path.isVideoName()
    val isSub = file.name.isSubtitleName() || file.path.isSubtitleName()
    val complete = !directPlay && file.isVerifiedComplete
    val canPlayVideo = isVideo && (directPlay || !file.uri.isNullOrBlank()) && (isSelected || complete)
    val canOpenOther = !isVideo && (isSelected || complete) && !file.uri.isNullOrBlank() && complete
    val statusText = when {
        !isSelected -> "Not selected"
        directPlay -> if (paused) "Direct Play · paused" else "Direct Play"
        file.uri.isNullOrBlank() -> "Not downloaded"
        complete -> "Ready"
        paused -> "Paused"
        else -> "Downloading"
    }

    Column(Modifier.fillMaxWidth().padding(vertical = 2.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Checkbox(
                checked = isSelected,
                onCheckedChange = { if (canToggle) onToggle() },
                enabled = canToggle,
                modifier = Modifier.size(32.dp),
            )
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(
                    file.name,
                    style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight.Medium),
                    color = colors.onSurface,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    if (directPlay) "${formatBytes(file.length)} · $statusText"
                    else "${formatBytes((file.length * file.progress).toLong())} of ${formatBytes(file.length)} · $statusText",
                    style = MaterialTheme.typography.labelSmall,
                    color = colors.onSurfaceVariant,
                )
                if (pairedSubtitles.isNotEmpty()) {
                    Text(
                        "Subtitles: ${pairedSubtitles.joinToString(", ")}",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.primary,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                } else if (isSub) {
                    Text(
                        "External subtitle",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.tertiary,
                    )
                }
            }
            if (isSelected && !directPlay) {
                FilledTonalButton(
                    onClick = if (focused) onClearFocus else onFocus,
                    shape = RoundedCornerShape(14.dp),
                    modifier = Modifier
                        .width(88.dp)
                        .heightIn(min = 48.dp).themedControl(),
                    contentPadding = PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                    colors = if (focused) {
                        ButtonDefaults.filledTonalButtonColors(
                            containerColor = colors.primary,
                            contentColor = colors.onPrimary,
                        )
                    } else {
                        ButtonDefaults.filledTonalButtonColors()
                    },
                ) {
                    Text(
                        if (focused) "Clear" else "Priority",
                        style = MaterialTheme.typography.labelMedium,
                        maxLines = 1,
                    )
                }
            }
            if (canPlayVideo || canOpenOther) {
                FilledTonalButton(
                    onClick = onPlay,
                    enabled = canPlay,
                    shape = RoundedCornerShape(14.dp),
                    modifier = Modifier.heightIn(min = 48.dp).themedControl(),
                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 0.dp),
                ) {
                    Text(
                        if (isVideo && file.playbackPositionMs > 0) "Resume ${playbackTime(file.playbackPositionMs)}" else if (isVideo) "Play" else "Open",
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
        }
        if (isSelected && !complete && !directPlay) {
            LinearProgressIndicator(
                progress = { file.progress.toFloat().coerceIn(0f, 1f) },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(2.dp)
                    .clip(MaterialTheme.shapes.extraSmall),
                color = colors.primary,
                trackColor = colors.surface,
            )
        }
        if (!directPlay) TextButton(onClick = onLocate, enabled = canToggle) { Text("Locate existing file") }
        if (canPlayVideo && file.playbackPositionMs > 0) TextButton(onClick = onStartOver, enabled = canPlay) { Text("Play from start") }
        HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.5f), modifier = Modifier.padding(top = 2.dp))
    }
}

@Composable
private fun TechnicalSection(
    entry: DownloadEntry,
    status: webtor.core.TorrentStatus?,
    telemetry: PieceTelemetry?,
    selectedCount: Int,
) {
    val context = LocalContext.current
    val clipboardManager = LocalClipboardManager.current
    val infoHash = status?.infoHash ?: entry.key.takeIf { it.length == 40 } ?: "—"
    val pieceDetails = telemetry?.let { "${it.totalPieces} pieces · ${formatBytes(it.pieceLength)} each" } ?: "—"

    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        SectionTitle("Technical information")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            TechnicalCard(
                label = "Files",
                value = "$selectedCount of ${entry.files.size} selected",
                modifier = Modifier.weight(1f),
            )
            TechnicalCard(
                label = "Pieces",
                value = pieceDetails,
                modifier = Modifier.weight(1f),
            )
        }
        TechnicalCard(
            label = "Destination",
            value = entry.destination,
            modifier = Modifier.fillMaxWidth(),
        )
        TechnicalCard(
            label = "Info hash (tap to copy)",
            value = infoHash,
            modifier = Modifier
                .fillMaxWidth()
                .clickable(enabled = infoHash != "—") {
                    clipboardManager.setText(AnnotatedString(infoHash))
                    Toast.makeText(context, "Info hash copied", Toast.LENGTH_SHORT).show()
                },
            monospace = true,
        )
    }
}

@Composable
private fun TechnicalCard(
    label: String,
    value: String,
    modifier: Modifier = Modifier,
    monospace: Boolean = false,
) {
    val colors = MaterialTheme.colorScheme
    Card(
        modifier = modifier,
        shape = RoundedCornerShape(10.dp),
        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
    ) {
        Column(Modifier.padding(horizontal = 8.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Text(label, style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
            Text(
                value,
                style = MaterialTheme.typography.bodySmall,
                color = colors.onSurface,
                fontFamily = if (monospace) FontFamily.Monospace else FontFamily.Default,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

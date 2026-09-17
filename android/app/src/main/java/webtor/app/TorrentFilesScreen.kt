package webtor.app

import android.os.SystemClock
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import webtor.core.PieceTelemetry

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
) {
    BackHandler(onBack = onBack)
    val colors = MaterialTheme.colorScheme
    val app = LocalContext.current.applicationContext as WebtorApp
    val client = app.engineClient
    val thumbnails = app.thumbnails
    var telemetry by remember(entry.key) { mutableStateOf<PieceTelemetry?>(null) }
    val previewVideo = firstPreviewVideo(entry)
    val previewUri = previewVideo?.uri
    val latestEntry by rememberUpdatedState(entry)
    var frames by remember(entry.key) { mutableStateOf(thumbnails.cached(entry)) }
    LaunchedEffect(entry.key, previewUri, entry.complete, entry.paused, entry.engineId) {
        if (previewUri.isNullOrBlank()) return@LaunchedEffect
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
    Scaffold(
        containerColor = colors.background,
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
                    containerColor = colors.background,
                    titleContentColor = colors.onBackground,
                    navigationIconContentColor = colors.onBackground,
                    actionIconContentColor = colors.onBackground,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 6.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            item { DownloadSummary(entry, error, frames) }
            item { TransferSection(entry, status) }
            item { TrackerDetailsSection(status) }
            item {
                PieceHeatGridSection(
                    map = telemetry,
                    frozen = entry.paused || entry.complete,
                    files = entry.files,
                )
            }
            item { SectionTitle("Files", "${entry.selected.size} of ${files.size} selected") }
            items(files, key = { it.index }) { file ->
                TorrentFileRow(
                    file = file,
                    isSelected = file.index in entry.selected,
                    paused = entry.paused,
                    canPlay = !busy && !entry.checking,
                    onPlay = { onPlay(file.index) },
                    onToggle = { onToggleFile(file.index) },
                    focused = entry.focusedFile == file.index,
                    onFocus = { onFocus(file.index) },
                    onClearFocus = onClearFocus,
                    canToggle = !busy && !entry.checking && entry.metadataReady,
                )
            }
            item { TechnicalSection(entry, status, telemetry, entry.selected.size) }
        }
    }
}

@Composable
private fun DownloadSummary(entry: DownloadEntry, error: String?, frames: List<android.graphics.Bitmap>?) {
    val colors = MaterialTheme.colorScheme
    val (label, labelBackground, labelColor) = when {
        entry.controlsBusy() -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        error != null || entry.error != null -> Triple("Needs attention", colors.errorContainer, colors.onErrorContainer)
        entry.complete -> Triple("Complete", colors.primaryContainer, colors.onPrimaryContainer)
        entry.paused || entry.engineId == null -> Triple("Paused", colors.tertiaryContainer, colors.onTertiaryContainer)
        else -> Triple(entry.stateLabel(), colors.primaryContainer, colors.onPrimaryContainer)
    }
    val video = entry.files.any { it.index in entry.selected && it.name.isVideoName() }
    val percent = (entry.progress.coerceIn(0f, 1f) * 100).toInt()
    ElevatedCard(
        shape = RoundedCornerShape(12.dp),
        colors = CardDefaults.elevatedCardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
        modifier = Modifier.fillMaxWidth(),
    ) {
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
                else "${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)} · $percent%",
                style = MaterialTheme.typography.bodySmall,
                color = colors.onSurfaceVariant,
            )
            if (!entry.complete) {
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
        }
    }
}

@Composable
private fun TransferSection(entry: DownloadEntry, status: webtor.core.TorrentStatus?) {
    val colors = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        SectionTitle("Transfer")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            TransferCard(
                label = "Download",
                value = formatSpeed(status?.downloadSpeed ?: 0),
                modifier = Modifier.weight(1f),
            )
            TransferCard(
                label = "Upload",
                value = formatSpeed(status?.uploadSpeed ?: 0),
                modifier = Modifier.weight(1f),
            )
            TransferCard(
                label = "ETA",
                value = formatEta(selectedEta(status, entry.downloaded, entry.total)) ?: "—",
                modifier = Modifier.weight(1f),
            )
        }
        Text(
            formatPeers(status?.numPeers ?: 0),
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

private fun selectedEta(status: webtor.core.TorrentStatus?, done: Long, total: Long): Long? =
    if (status == null || status.downloadSpeed <= 0 || done >= total) null else ((total - done).toDouble() / status.downloadSpeed * 1000).toLong()

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
) {
    val colors = MaterialTheme.colorScheme
    val isVideo = file.name.isVideoName() || file.path.isVideoName()
    val complete = file.isVerifiedComplete
    val canPlayVideo = isVideo && !file.uri.isNullOrBlank() && (isSelected || complete)
    val canOpenOther = !isVideo && (isSelected || complete) && !file.uri.isNullOrBlank() && complete
    val statusText = when {
        !isSelected -> "Not selected"
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
                    "${formatBytes((file.length * file.progress).toLong())} of ${formatBytes(file.length)} · $statusText",
                    style = MaterialTheme.typography.labelSmall,
                    color = colors.onSurfaceVariant,
                )
            }
            if (isSelected) {
                FilledTonalButton(
                    onClick = if (focused) onClearFocus else onFocus,
                    shape = RoundedCornerShape(14.dp),
                    modifier = Modifier
                        .width(88.dp)
                        .height(30.dp),
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
                    modifier = Modifier.height(30.dp),
                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 0.dp),
                ) {
                    Text(
                        if (isVideo) "Play" else "Open",
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
        }
        if (isSelected && !complete) {
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

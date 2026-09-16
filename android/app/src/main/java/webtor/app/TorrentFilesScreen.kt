package webtor.app

import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
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
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
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
    onPause: () -> Unit,
    onResume: () -> Unit,
    onDelete: () -> Unit,
) {
    BackHandler(onBack = onBack)
    val colors = MaterialTheme.colorScheme
    val app = LocalContext.current.applicationContext as WebtorApp
    val client = app.engineClient
    val thumbnails = app.thumbnails
    var telemetry by remember(entry.key) { mutableStateOf<PieceTelemetry?>(null) }
    val previewVideo = firstPreviewVideo(entry)
    val previewUri = previewVideo?.uri
    val previewBucket = previewUpdateBucket(previewVideo?.progress ?: 0.0, entry.complete)
    val latestEntry by rememberUpdatedState(entry)
    var frames by remember(entry.key) { mutableStateOf(thumbnails.cached(entry)) }
    LaunchedEffect(entry.key, previewUri, entry.complete, previewBucket, entry.paused, entry.engineId) {
        if (previewUri.isNullOrBlank()) return@LaunchedEffect
        while (true) {
            val current = latestEntry
            val next = thumbnails.previews(current)
            if (next.isNotEmpty()) frames = next
            else if (frames == null) frames = emptyList()
            val live = !current.complete && !current.paused && current.engineId != null
            if (!frames.isNullOrEmpty() || !live) break
            delay(1000)
        }
    }
    val lifecycleOwner = LocalLifecycleOwner.current
    LaunchedEffect(entry.key, entry.engineId, entry.complete, lifecycleOwner) {
        val engineId = entry.engineId ?: return@LaunchedEffect
        var eventVersion = -1L
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                val nextVersion = withContext(Dispatchers.IO) {
                    app.engine.awaitChange(eventVersion)
                }
                if (nextVersion == eventVersion) continue
                eventVersion = nextVersion
                val next = withContext(Dispatchers.IO) {
                    runCatching { client.pieces(engineId, 256) }.getOrNull()
                }
                if (next != null) telemetry = next
                if (entry.complete) break
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
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            item { DownloadSummary(entry, error, frames) }
            item { SectionTitle("Files", "${entry.selected.size} of ${files.size} selected") }
            items(files, key = { it.index }) { file ->
                TorrentFileRow(
                    file = file,
                    isSelected = file.index in entry.selected,
                    paused = entry.paused,
                    canPlay = !busy && !entry.checking,
                    onPlay = { onPlay(file.index) },
                )
            }
            item { TransferSection(entry, status) }
            item { PiecesSection(telemetry, entry.paused || entry.complete) }
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
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.elevatedCardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (video) VideoPreviewPager(entry, frames, Modifier.fillMaxWidth().aspectRatio(16f / 9f), large = true)
            Text(
                entry.title,
                style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.Bold),
                color = colors.onSurface,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                label,
                Modifier
                    .background(labelBackground, RoundedCornerShape(8.dp))
                    .padding(horizontal = 8.dp, vertical = 4.dp),
                style = MaterialTheme.typography.labelSmall.copy(fontWeight = FontWeight.SemiBold),
                color = labelColor,
            )
            Text(
                if (entry.checking) "Checking saved data · ${entry.checkPercent}%"
                else "${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)} · $percent%",
                style = MaterialTheme.typography.bodyMedium,
                color = colors.onSurfaceVariant,
            )
            if (!entry.complete) {
                LinearProgressIndicator(
                    progress = { entry.progress.coerceIn(0f, 1f) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(4.dp)
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
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionTitle("Transfer")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
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
            style = MaterialTheme.typography.bodySmall,
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
        shape = RoundedCornerShape(12.dp),
        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
    ) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                label,
                style = MaterialTheme.typography.labelSmall,
                color = colors.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                value,
                style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.Bold),
                color = colors.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun SectionTitle(title: String, subtitle: String? = null) {
    val colors = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(
            title,
            style = MaterialTheme.typography.titleMedium,
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
private fun PiecesSection(map: PieceTelemetry?, frozen: Boolean) {
    val colors = MaterialTheme.colorScheme
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionTitle("Pieces")
        Card(
            modifier = Modifier.fillMaxWidth(),
            shape = RoundedCornerShape(12.dp),
            colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
        ) {
            if (map == null || map.buckets.isEmpty()) {
                Text(
                    "Map unavailable",
                    Modifier.padding(14.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.onSurfaceVariant,
                )
            } else {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    PieceCanvas(map, frozen, Modifier.fillMaxWidth().height(24.dp))
                    Text(
                        "Missing · Receiving · Verified · Excluded",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

@Composable
private fun PieceCanvas(map: PieceTelemetry, frozen: Boolean, modifier: Modifier) {
    val colors = MaterialTheme.colorScheme
    Canvas(modifier) {
        val gap = 1.5f
        val width = (size.width - gap * (map.buckets.size - 1).coerceAtLeast(0)) / map.buckets.size.coerceAtLeast(1)
        map.buckets.forEachIndexed { index, b ->
            val color = when {
                b.verified == b.total -> colors.primary
                !frozen && b.receiving > 0 -> colors.tertiary
                b.selected == 0 -> colors.outlineVariant
                else -> colors.surfaceVariant
            }
            drawRoundRect(
                color = color,
                topLeft = Offset(index * (width + gap), 0f),
                size = Size(width, size.height),
                cornerRadius = CornerRadius(2f, 2f),
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
) {
    val colors = MaterialTheme.colorScheme
    val isVideo = file.name.isVideoName() || file.path.isVideoName()
    val complete = file.length == 0L || file.progress >= 1.0
    val canPlayVideo = isVideo && isSelected && !file.uri.isNullOrBlank()
    val canOpenOther = !isVideo && isSelected && !file.uri.isNullOrBlank() && complete
    val statusText = when {
        !isSelected -> "Not selected"
        file.uri.isNullOrBlank() -> "Not downloaded"
        complete -> "Ready"
        paused -> "Paused"
        else -> "Downloading"
    }

    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    file.name,
                    style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium),
                    color = colors.onSurface,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    "${formatBytes((file.length * file.progress).toLong())} of ${formatBytes(file.length)} · $statusText",
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.onSurfaceVariant,
                )
            }
            if (canPlayVideo || canOpenOther) {
                FilledTonalButton(
                    onClick = onPlay,
                    enabled = canPlay,
                    shape = RoundedCornerShape(16.dp),
                    modifier = Modifier.heightIn(min = 34.dp),
                    contentPadding = PaddingValues(horizontal = 14.dp, vertical = 4.dp),
                ) {
                    Text(if (isVideo) "Play" else "Open")
                }
            }
        }
        if (isSelected && !complete) {
            LinearProgressIndicator(
                progress = { file.progress.toFloat().coerceIn(0f, 1f) },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(2.5.dp)
                    .clip(MaterialTheme.shapes.extraSmall),
                color = colors.primary,
                trackColor = colors.surface,
            )
        }
        HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.5f), modifier = Modifier.padding(top = 4.dp))
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

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionTitle("Technical information")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
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
        shape = RoundedCornerShape(12.dp),
        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
    ) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
            Text(
                value,
                style = MaterialTheme.typography.bodyMedium,
                color = colors.onSurface,
                fontFamily = if (monospace) FontFamily.Monospace else FontFamily.Default,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

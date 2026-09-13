package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import webtor.core.EngineClient
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
    val client = remember { EngineClient(NodeHost.DEFAULT_CTL_PORT) }
    val thumbnails = (LocalContext.current.applicationContext as WebtorApp).thumbnails
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
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                val next = withContext(Dispatchers.IO) {
                    runCatching { client.pieces(engineId, 256) }.getOrNull()
                }
                if (next != null) telemetry = next
                if (entry.complete) break
                delay(1000)
            }
        }
    }
    val files = entry.files.filter { it.index in entry.selected }
    val status = entry.status
    val busy = entry.controlsBusy()
    Scaffold(containerColor = MaterialTheme.colorScheme.background, topBar = {
        TopAppBar(
            title = { Text("Download details", style = MaterialTheme.typography.titleLarge) },
            navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
            actions = {
                when {
                    busy || entry.complete -> Unit
                    entry.paused || entry.engineId == null -> TextButton(onClick = onResume) { Text("Resume") }
                    else -> TextButton(onClick = onPause) { Text("Pause") }
                }
                TextButton(onClick = onDelete, enabled = !busy) { Text("Remove") }
            },
            colors = TopAppBarDefaults.topAppBarColors(
                containerColor = MaterialTheme.colorScheme.background,
                titleContentColor = MaterialTheme.colorScheme.onBackground,
            ),
        )
    }) { padding ->
        LazyColumn(
            Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            item { DownloadSummary(entry, error, frames) }
            item { TransferSection(entry, status) }
            item { PiecesSection(telemetry, entry.paused || entry.complete) }
            item { SectionTitle("Files", "${files.size} selected") }
            items(files, key = { it.index }) { file -> TorrentFileRow(file, entry.paused) { onPlay(file.index) } }
            item {
                TechnicalSection(entry, status, telemetry, files.size)
            }
        }
    }
}

@Composable
private fun DownloadSummary(entry: DownloadEntry, error: String?, frames: List<android.graphics.Bitmap>?) {
    val colors = MaterialTheme.colorScheme
    val (label, labelBackground, labelColor) = when {
        error != null || entry.error != null -> Triple("Needs attention", colors.errorContainer, colors.onErrorContainer)
        entry.complete -> Triple("Complete", colors.primaryContainer, colors.onPrimaryContainer)
        entry.paused || entry.engineId == null -> Triple("Paused", colors.tertiaryContainer, colors.onTertiaryContainer)
        else -> Triple("Downloading", colors.primaryContainer, colors.onPrimaryContainer)
    }
    val video = entry.files.any { it.index in entry.selected && it.name.isVideoName() }
    Surface(shape = RoundedCornerShape(16.dp), color = colors.surfaceVariant.copy(alpha = .48f)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (video) VideoPreviewPager(entry, frames, Modifier.fillMaxWidth().aspectRatio(16f / 9f), large = true)
            Text(entry.title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(label, Modifier.background(labelBackground, RoundedCornerShape(6.dp)).padding(horizontal = 8.dp, vertical = 4.dp), style = MaterialTheme.typography.labelSmall, color = labelColor)
            Text("${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)}", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
            LinearProgressIndicator(
                progress = { entry.progress.coerceIn(0f, 1f) },
                modifier = Modifier.fillMaxWidth().height(4.dp),
                color = colors.primary,
                trackColor = colors.surface,
            )
            error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = colors.error) }
        }
    }
}

@Composable
private fun TransferSection(entry: DownloadEntry, status: webtor.core.TorrentStatus?) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionTitle("Transfer")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TransferMetric("Download", formatSpeed(status?.downloadSpeed ?: 0), Modifier.weight(1f))
            TransferMetric("Upload", formatSpeed(status?.uploadSpeed ?: 0), Modifier.weight(1f))
            TransferMetric("ETA", formatEta(selectedEta(status, entry.downloaded, entry.total)) ?: "Estimating…", Modifier.weight(1f))
        }
        Text(formatPeers(status?.numPeers ?: 0), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable private fun TransferMetric(label: String, value: String, modifier: Modifier = Modifier) =
    Surface(modifier, shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = .48f)) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }

@Composable private fun SectionTitle(title: String, subtitle: String? = null) =
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
        subtitle?.let { Text(it, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    }

private fun selectedEta(status: webtor.core.TorrentStatus?, done: Long, total: Long): Long? = if (status == null || status.downloadSpeed <= 0 || done >= total) null else ((total - done).toDouble() / status.downloadSpeed * 1000).toLong()

@Composable private fun PiecesSection(map: PieceTelemetry?, frozen: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionTitle("Pieces")
        Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = .48f)) {
            if (map == null || map.buckets.isEmpty()) Text("Map unavailable", Modifier.padding(14.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) else {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(7.dp)) {
                    PieceCanvas(map, frozen, Modifier.fillMaxWidth().height(30.dp))
                    Text("Missing · Receiving · Verified · Excluded", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

@Composable private fun PieceCanvas(map: PieceTelemetry, frozen: Boolean, modifier: Modifier) {
    val colors = MaterialTheme.colorScheme
    Canvas(modifier) {
        val gap = 2f
        val width = (size.width - gap * (map.buckets.size - 1).coerceAtLeast(0)) / map.buckets.size.coerceAtLeast(1)
        map.buckets.forEachIndexed { index, b ->
            val color = when { b.verified == b.total -> colors.primary; !frozen && b.receiving > 0 -> colors.tertiary; b.selected == 0 -> colors.outlineVariant; else -> colors.surfaceVariant }
            drawRoundRect(color, androidx.compose.ui.geometry.Offset(index * (width + gap), 0f), androidx.compose.ui.geometry.Size(width, size.height), CornerRadius(3f, 3f))
        }
    }
}

@Composable private fun TorrentFileRow(file: SavedFile, paused: Boolean, onPlay: () -> Unit) {
    val complete = file.length == 0L || file.progress >= 1.0
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(7.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(file.name, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text("${formatBytes((file.length * file.progress).toLong())} of ${formatBytes(file.length)} · ${when { file.uri == null -> "Not downloaded"; complete -> "Ready"; paused -> "Paused"; else -> "Downloading" }}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            TextButton(onClick = onPlay, enabled = file.uri != null) { Text(if (file.name.isVideoName()) "Play" else "Open") }
        }
        LinearProgressIndicator({ file.progress.toFloat().coerceIn(0f, 1f) }, Modifier.fillMaxWidth().height(3.dp))
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant, modifier = Modifier.padding(top = 10.dp))
}

@Composable
private fun TechnicalSection(entry: DownloadEntry, status: webtor.core.TorrentStatus?, telemetry: PieceTelemetry?, selectedCount: Int) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionTitle("Technical information")
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TransferMetric("Files", "$selectedCount of ${entry.files.size} selected", Modifier.weight(1f))
            TransferMetric(
                "Pieces",
                telemetry?.let { "${it.totalPieces} · ${formatBytes(it.pieceLength)} each" } ?: "—",
                Modifier.weight(1f),
            )
        }
        TechnicalFact("Location", entry.destination)
        TechnicalFact("Info hash", status?.infoHash ?: "—")
    }
}

@Composable private fun TechnicalFact(label: String, value: String) =
    Surface(Modifier.fillMaxWidth(), shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = .48f)) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, style = MaterialTheme.typography.bodyMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }

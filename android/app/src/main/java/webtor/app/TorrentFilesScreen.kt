package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import webtor.core.EngineClient
import webtor.core.PieceTelemetry

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TorrentFilesScreen(entry: DownloadEntry, error: String?, onBack: () -> Unit, onPlay: (Int) -> Unit) {
    BackHandler(onBack = onBack)
    var telemetry by remember(entry.key) { mutableStateOf<PieceTelemetry?>(null) }
    LaunchedEffect(entry.key, entry.engineId, entry.paused, entry.complete) {
        while (true) {
            if (entry.engineId != null && !entry.complete) telemetry = runCatching { EngineClient(NodeHost.DEFAULT_CTL_PORT).pieces(entry.engineId, 256) }.getOrNull()
            delay(1000)
        }
    }
    val files = entry.files.filter { it.index in entry.selected }
    val status = entry.status
    Scaffold(containerColor = MaterialTheme.colorScheme.background, topBar = {
        TopAppBar(title = { Text("Details") }, navigationIcon = { TextButton(onClick = onBack) { Text("Back") } }, colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background))
    }) { padding ->
        LazyColumn(Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(horizontal = 24.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(entry.title, style = MaterialTheme.typography.headlineSmall)
                    Text(if (entry.paused) "Paused" else if (entry.complete) "Completed" else "Downloading", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text("${formatBytes(entry.downloaded)} / ${formatBytes(entry.total)} selected")
                    LinearProgressIndicator({ entry.progress }, Modifier.fillMaxWidth().height(4.dp))
                    error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("Transfer", style = MaterialTheme.typography.titleMedium)
                    Text("Download ${formatSpeed(status?.downloadSpeed ?: 0)} · Upload ${formatSpeed(status?.uploadSpeed ?: 0)}")
                    Text("${formatPeers(status?.numPeers ?: 0)} · ETA ${formatEta(selectedEta(status, entry.downloaded, entry.total)) ?: "Estimating…"}", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            item { PiecesSection(telemetry, entry.paused || entry.complete) }
            item { Text("Selected files", style = MaterialTheme.typography.titleMedium) }
            items(files, key = { it.index }) { file -> TorrentFileRow(file, entry.paused) { onPlay(file.index) } }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("Technical information", style = MaterialTheme.typography.titleMedium)
                    Text("Info hash: ${status?.infoHash ?: "Unavailable"}")
                    Text("Destination: ${entry.destination}")
                    Text("Files: ${files.size} selected / ${entry.files.size} total")
                    Text("Pieces: ${telemetry?.totalPieces ?: "Unavailable"} · Piece size: ${telemetry?.pieceLength?.let(::formatBytes) ?: "Unavailable"}")
                    Text("Tracker diagnostics: unavailable")
                }
            }
        }
    }
}

private fun selectedEta(status: webtor.core.TorrentStatus?, done: Long, total: Long): Long? = if (status == null || status.downloadSpeed <= 0 || done >= total) null else ((total - done).toDouble() / status.downloadSpeed * 1000).toLong()

@Composable private fun PiecesSection(map: PieceTelemetry?, frozen: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Pieces", style = MaterialTheme.typography.titleMedium)
        if (map == null) Text("Map unavailable", color = MaterialTheme.colorScheme.onSurfaceVariant) else {
            PieceCanvas(map, frozen, Modifier.fillMaxWidth().height(42.dp))
            Text("Missing · Receiving · Verified · Excluded", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
    Column(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(file.name, style = MaterialTheme.typography.titleMedium)
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("${formatBytes((file.length * file.progress).toLong())} / ${formatBytes(file.length)} · ${when { file.uri == null -> "Not downloaded"; complete -> "Done"; paused -> "Paused"; else -> "Downloading" }}", Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextButton(onClick = onPlay, enabled = file.uri != null) { Text(if (file.name.isVideoName()) "Play" else "Open") }
        }
        LinearProgressIndicator({ file.progress.toFloat().coerceIn(0f, 1f) }, Modifier.fillMaxWidth().height(2.dp))
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outline)
}

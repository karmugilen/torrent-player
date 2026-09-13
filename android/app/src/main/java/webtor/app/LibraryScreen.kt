package webtor.app

import androidx.compose.foundation.Image
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.runtime.getValue
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(
    state: UiState,
    onAdd: () -> Unit,
    onSettings: () -> Unit,
    onPause: (DownloadEntry) -> Unit,
    onResume: (DownloadEntry) -> Unit,
    onPlay: (DownloadEntry) -> Unit,
    onOpen: (DownloadEntry) -> Unit,
    onDelete: (DownloadEntry) -> Unit,
    onRetry: (DownloadEntry) -> Unit,
    onMessageShown: () -> Unit,
) {
    val context = LocalContext.current
    val thumbnailRepository = remember(context) { ThumbnailRepository(context) }
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(state.message) {
        val text = state.message ?: return@LaunchedEffect
        snackbar.showSnackbar(text)
        onMessageShown()
    }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text("Torrent Player", style = MaterialTheme.typography.headlineMedium) },
                actions = {
                    TextButton(onClick = onAdd) { Text("Add") }
                    TextButton(onClick = onSettings) { Text("Settings") }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                    titleContentColor = MaterialTheme.colorScheme.onBackground,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
            contentPadding = PaddingValues(horizontal = 24.dp, vertical = 8.dp),
        ) {
            if (!state.engineReady || state.restoring) {
                item {
                    Text(
                        state.statusLine,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(bottom = 16.dp),
                    )
                }
            }
            state.error?.takeIf { !state.addSheetOpen }?.let { err ->
                item {
                    Text(
                        err,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(bottom = 16.dp),
                    )
                }
            }
            state.loadWarning?.let { warn ->
                item {
                    Text(
                        warn,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(bottom = 16.dp),
                    )
                }
            }
            val visible = state.visibleLibrary()
            if (visible.isEmpty()) {
                item {
                    Column(Modifier.padding(top = 48.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Nothing here.", style = MaterialTheme.typography.headlineSmall)
                        Text(
                            "Add a magnet or a torrent file.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            items(visible, key = { it.key }) { entry ->
                TorrentRow(
                    entry = entry,
                    thumbnail = remember(entry.key, entry.complete, entry.files) { thumbnailRepository.thumbnail(entry) },
                    onPause = { onPause(entry) },
                    onResume = { onResume(entry) },
                    onPlay = { onPlay(entry) },
                    onOpen = { onOpen(entry) },
                    onDelete = { onDelete(entry) },
                    onRetry = { onRetry(entry) },
                )
            }
        }
    }
}

@Composable
private fun TorrentRow(
    entry: DownloadEntry,
    thumbnail: android.graphics.Bitmap?,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onPlay: () -> Unit,
    onOpen: () -> Unit,
    onDelete: () -> Unit,
    onRetry: () -> Unit,
) {
    val status = entry.status
    val progress by animateFloatAsState(entry.progress, tween(500), label = "downloadProgress")
    Column(Modifier.fillMaxWidth().clickable(onClick = onOpen).padding(vertical = 18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        thumbnail?.let { Image(it.asImageBitmap(), contentDescription = null, modifier = Modifier.fillMaxWidth().height(180.dp)) }
        Text(
            entry.title,
            style = MaterialTheme.typography.titleMedium,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            StatusMark(entry)
            Text(
                buildString {
                    append(formatBytes(entry.downloaded))
                    append(" / ")
                    append(formatBytes(entry.total))
                    if (status != null && entry.engineId != null && !entry.complete && !entry.paused) {
                        append("  ·  ")
                        append(formatSpeed(status.downloadSpeed))
                        append("  ·  ")
                        append(formatPeers(status.numPeers))
                    }
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        LinearProgressIndicator(
            progress = { progress.coerceIn(0f, 1f) },
            modifier = Modifier.fillMaxWidth().height(2.dp),
            color = MaterialTheme.colorScheme.primary,
            trackColor = MaterialTheme.colorScheme.outline,
        )
        entry.error?.let {
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
        }
        Row {
            when {
                entry.complete -> Unit
                entry.error != null -> TextButton(onClick = onRetry) { Text("Retry") }
                entry.paused || entry.engineId == null -> TextButton(onClick = onResume) { Text("Resume") }
                else -> TextButton(onClick = onPause) { Text("Pause") }
            }
            TextButton(onClick = onOpen) { Text("Details") }
            TextButton(onClick = onPlay) { Text(if (entry.selected.size > 1) "Files" else "Play") }
            TextButton(onClick = onDelete) { Text("Remove") }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outline, thickness = 1.dp)
}

@Composable
private fun StatusMark(entry: DownloadEntry) {
    val colors = MaterialTheme.colorScheme
    val (label, bg, fg) = when {
        entry.error != null -> Triple("Error", colors.errorContainer, colors.onErrorContainer)
        entry.complete -> Triple("Done", colors.primaryContainer, colors.onPrimaryContainer)
        entry.paused || entry.engineId == null -> Triple("Paused", colors.tertiaryContainer, colors.onTertiaryContainer)
        else -> Triple("Downloading", colors.primaryContainer, colors.onPrimaryContainer)
    }
    Text(
        label.uppercase(),
        modifier = Modifier
            .background(bg, RoundedCornerShape(50))
            .padding(horizontal = 8.dp, vertical = 3.dp),
        style = MaterialTheme.typography.labelSmall,
        color = fg,
    )
}

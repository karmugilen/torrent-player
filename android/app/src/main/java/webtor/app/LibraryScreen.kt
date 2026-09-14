package webtor.app

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

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
    val repository = (LocalContext.current.applicationContext as WebtorApp).thumbnails
    val snackbar = remember { SnackbarHostState() }
    var query by remember { mutableStateOf("") }
    LaunchedEffect(state.message) {
        state.message?.let { snackbar.showSnackbar(it); onMessageShown() }
    }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        "Downloads",
                        style = MaterialTheme.typography.titleLarge.copy(
                            fontWeight = FontWeight.Bold,
                        ),
                    )
                },
                actions = {
                    IconButton(onClick = onSettings) {
                        Icon(
                            Icons.Default.Settings,
                            contentDescription = "Settings",
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                    titleContentColor = MaterialTheme.colorScheme.onBackground,
                    actionIconContentColor = MaterialTheme.colorScheme.onBackground,
                ),
            )
        },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = onAdd,
                icon = { Icon(Icons.Default.Add, contentDescription = null) },
                text = {
                    Text(
                        "Add download",
                        style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold),
                    )
                },
                containerColor = MaterialTheme.colorScheme.primaryContainer,
                contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (!state.engineReady || state.restoring) item { LibraryNotice(state.statusLine) }
            state.error?.takeIf { !state.addSheetOpen }?.let { item { LibraryNotice(it, true) } }
            state.loadWarning?.let { item { LibraryNotice(it, true) } }
            if (state.library.isNotEmpty()) item {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(bottom = 2.dp),
                    singleLine = true,
                    placeholder = {
                        Text(
                            "Search downloads",
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    },
                    leadingIcon = {
                        Icon(
                            Icons.Default.Search,
                            contentDescription = null,
                            modifier = Modifier.size(20.dp),
                        )
                    },
                    trailingIcon = if (query.isNotEmpty()) ({
                        IconButton(onClick = { query = "" }) {
                            Icon(
                                Icons.Default.Clear,
                                contentDescription = "Clear search",
                                modifier = Modifier.size(20.dp),
                            )
                        }
                    }) else null,
                    shape = RoundedCornerShape(12.dp),
                )
            }
            val visible = state.visibleLibrary().filter { query.isBlank() || it.title.contains(query.trim(), ignoreCase = true) }
            if (visible.isEmpty()) item {
                Box(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 96.dp, start = 16.dp, end = 16.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            text = "No downloads yet",
                            style = MaterialTheme.typography.headlineSmall,
                            textAlign = TextAlign.Center,
                        )
                        Text(
                            text = "Add a magnet link or torrent file to get started.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            textAlign = TextAlign.Center,
                        )
                    }
                }
            }
            items(visible, key = { it.key }) { entry ->
                TorrentRow(
                    entry = entry,
                    repository = repository,
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
private fun LibraryNotice(message: String, error: Boolean = false) = Text(
    message,
    color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.tertiary,
    style = MaterialTheme.typography.bodyMedium,
    modifier = Modifier.padding(vertical = 10.dp),
)

@Composable
private fun TorrentRow(
    entry: DownloadEntry,
    repository: ThumbnailRepository,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onPlay: () -> Unit,
    onOpen: () -> Unit,
    onDelete: () -> Unit,
    onRetry: () -> Unit,
) {
    val busy = entry.controlsBusy()
    val progress by animateFloatAsState(entry.progress, tween(200), label = "downloadProgress")
    val previewVideo = firstPreviewVideo(entry)
    val previewUri = previewVideo?.uri
    val previewBucket = previewUpdateBucket(previewVideo?.progress ?: 0.0, entry.complete)
    val latestEntry by rememberUpdatedState(entry)
    var frames by remember(entry.key) { mutableStateOf(repository.cached(entry)) }
    LaunchedEffect(entry.key, previewUri, entry.complete, previewBucket, entry.paused, entry.engineId) {
        if (previewUri.isNullOrBlank()) return@LaunchedEffect
        while (true) {
            val current = latestEntry
            val next = repository.previews(current)
            if (next.isNotEmpty()) frames = next
            else if (frames == null) frames = emptyList()
            val live = !current.complete && !current.paused && current.engineId != null
            if (!frames.isNullOrEmpty() || !live) break
            delay(1000)
        }
    }
    ElevatedCard(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(onClick = onOpen),
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
        ),
    ) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.Top,
            ) {
                if (entry.files.any { it.index in entry.selected && it.name.isVideoName() }) {
                    VideoPreviewPager(
                        entry = entry,
                        frames = frames,
                        modifier = Modifier
                            .width(128.dp)
                            .aspectRatio(16f / 9f),
                    )
                }
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            text = entry.title,
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        StatusMark(entry)
                    }
                    Text(
                        text = entryTelemetry(entry),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            if (!entry.complete) {
                LinearProgressIndicator(
                    progress = { progress.coerceIn(0f, 1f) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(4.dp),
                    color = MaterialTheme.colorScheme.primary,
                    trackColor = MaterialTheme.colorScheme.surface,
                )
            }
            entry.error?.let {
                Text(
                    text = it,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val selectedVideos = entry.files.filter { it.index in entry.selected && (it.name.isVideoName() || it.path.isVideoName()) }
                val isMultiVideo = selectedVideos.size > 1
                val canPlay = entry.canPlay()
                val isVideo = mediaType(entry) == "Video"

                if (canPlay || entry.complete) {
                    Button(
                        onClick = if (isMultiVideo) onOpen else if (canPlay) onPlay else onOpen,
                        enabled = !busy,
                    ) {
                        Icon(Icons.Default.PlayArrow, contentDescription = null)
                        Spacer(Modifier.width(4.dp))
                        Text(if (isVideo) "Play" else "Open")
                    }
                }

                when {
                    busy -> OutlinedButton(
                        onClick = {},
                        enabled = false,
                    ) {
                        Text(entry.stateLabel())
                    }
                    entry.error != null -> OutlinedButton(
                        onClick = onRetry,
                    ) {
                        Text("Retry")
                    }
                    entry.complete -> Unit
                    entry.paused || entry.engineId == null -> {
                        val hasAnyUri = entry.files.any { it.index in entry.selected && !it.uri.isNullOrBlank() }
                        OutlinedButton(
                            onClick = onResume,
                        ) {
                            Text(if (!hasAnyUri) "Download" else "Resume")
                        }
                    }
                    else -> OutlinedButton(
                        onClick = onPause,
                    ) {
                        Text("Pause")
                    }
                }

                Spacer(Modifier.weight(1f))

                IconButton(
                    onClick = onDelete,
                    enabled = !busy,
                ) {
                    Icon(
                        Icons.Default.Delete,
                        contentDescription = "Remove download",
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private fun selectedEta(status: webtor.core.TorrentStatus?, done: Long, total: Long): Long? =
    if (status == null || status.downloadSpeed <= 0 || done >= total) null else ((total - done).toDouble() / status.downloadSpeed * 1000).toLong()

private fun entryTelemetry(entry: DownloadEntry): String = buildString {
    append("${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)}")
    val status = entry.status
    if (!entry.complete && !entry.paused && !entry.controlsBusy() && entry.engineId != null && status != null) {
        append(" · ${formatSpeed(status.downloadSpeed)} · ${formatPeers(status.numPeers)}")
        formatEta(selectedEta(status, entry.downloaded, entry.total))?.let { eta ->
            append(" · $eta")
        }
    }
}

@Composable
fun VideoPreviewPager(
    entry: DownloadEntry,
    frames: List<android.graphics.Bitmap>?,
    modifier: Modifier = Modifier,
    large: Boolean = false,
) {
    val shape = RoundedCornerShape(if (large) 12.dp else 8.dp)
    val ready = frames.orEmpty()
    if (ready.isEmpty()) {
        Box(
            modifier = modifier
                .clip(shape)
                .background(MaterialTheme.colorScheme.surfaceVariant),
        )
        return
    }
    val pager = rememberPagerState(pageCount = { ready.size })
    HorizontalPager(
        state = pager,
        modifier = modifier.clip(shape),
    ) { page ->
        Image(
            ready[page].asImageBitmap(),
            contentDescription = "${entry.title}, preview ${page + 1} of ${ready.size}",
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize(),
        )
    }
}

@Composable
private fun StatusMark(entry: DownloadEntry) {
    val colors = MaterialTheme.colorScheme
    val (label, bg, fg) = when {
        entry.error != null -> Triple("Needs attention", colors.errorContainer, colors.onErrorContainer)
        entry.complete -> Triple("Complete", colors.primaryContainer, colors.onPrimaryContainer)
        entry.paused || entry.engineId == null -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        entry.controlsBusy() -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        else -> Triple("Downloading", colors.primaryContainer, colors.onPrimaryContainer)
    }
    Text(
        text = label,
        modifier = Modifier
            .background(bg, RoundedCornerShape(6.dp))
            .padding(horizontal = 8.dp, vertical = 4.dp),
        style = MaterialTheme.typography.labelSmall,
        color = fg,
    )
}

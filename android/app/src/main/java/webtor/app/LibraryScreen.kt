package webtor.app

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
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
                title = { Text("Downloads", style = MaterialTheme.typography.headlineMedium) },
                actions = {
                    TextButton(onClick = onAdd) { Text("Add") }
                    TextButton(onClick = onSettings) { Text("Settings") }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                    titleContentColor = MaterialTheme.colorScheme.onBackground,
                    actionIconContentColor = MaterialTheme.colorScheme.onBackground,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (!state.engineReady || state.restoring) item { LibraryNotice(state.statusLine) }
            state.error?.takeIf { !state.addSheetOpen }?.let { item { LibraryNotice(it, true) } }
            state.loadWarning?.let { item { LibraryNotice(it, true) } }
            if (state.library.isNotEmpty()) item {
                OutlinedTextField(value = query, onValueChange = { query = it }, modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp), singleLine = true, label = { Text("Search downloads") }, trailingIcon = if (query.isNotEmpty()) ({ TextButton(onClick = { query = "" }) { Text("Clear") } }) else null)
            }
            val visible = state.visibleLibrary().filter { query.isBlank() || it.title.contains(query.trim(), ignoreCase = true) }
            if (visible.isEmpty()) item {
                Column(Modifier.padding(top = 64.dp, start = 4.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(if (query.isBlank()) "Your downloads will appear here" else "No matching downloads", style = MaterialTheme.typography.headlineSmall)
                    Text(if (query.isBlank()) "Add a magnet link or torrent file to get started." else "Try a different name or clear your search.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    if (query.isBlank()) Button(onClick = onAdd, modifier = Modifier.padding(top = 8.dp)) { Text("Add download") }
                }
            }
            items(visible, key = { it.key }) { entry ->
                TorrentRow(entry, repository, { onPause(entry) }, { onResume(entry) }, { onPlay(entry) }, { onOpen(entry) }, { onDelete(entry) }, { onRetry(entry) })
            }
        }
    }
}

@Composable private fun LibraryNotice(message: String, error: Boolean = false) = Text(
    message,
    color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
    style = MaterialTheme.typography.bodyMedium,
    modifier = Modifier.padding(vertical = 10.dp),
)

@OptIn(ExperimentalLayoutApi::class)
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
    val progress by animateFloatAsState(entry.progress, tween(500), label = "downloadProgress")
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
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = .48f),
    ) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (entry.files.any { it.index in entry.selected && it.name.isVideoName() }) {
                    VideoPreviewPager(entry, frames, Modifier.width(132.dp).aspectRatio(16f / 9f))
                }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(7.dp)) {
                    Text(entry.title, style = MaterialTheme.typography.titleMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
                    StatusMark(entry)
                    Text(entryMeta(entry), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
            if (!entry.complete) LinearProgressIndicator(
                progress = { progress.coerceIn(0f, 1f) },
                modifier = Modifier.fillMaxWidth().height(3.dp),
                color = MaterialTheme.colorScheme.primary,
                trackColor = MaterialTheme.colorScheme.surface,
            )
            entry.error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium) }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                val manyVideos = entry.selectedVideoCount() > 1
                if (manyVideos) {
                    TextButton(onClick = onOpen, enabled = !busy) { Text("Episodes") }
                } else if (entry.canPlay()) {
                    TextButton(onClick = onPlay, enabled = !busy) {
                        Text(if (mediaType(entry) == "Video") "Play" else "Open")
                    }
                }
                when {
                    busy -> TextButton(onClick = {}, enabled = false) { Text(entry.stateLabel()) }
                    entry.error != null -> TextButton(onClick = onRetry) { Text("Retry") }
                    entry.complete -> Unit
                    entry.paused || entry.engineId == null -> TextButton(onClick = onResume) { Text("Resume") }
                    else -> TextButton(onClick = onPause) { Text("Pause") }
                }
                if (!manyVideos) TextButton(onClick = onOpen, enabled = !busy) { Text("Details") }
                TextButton(onClick = onDelete, enabled = !busy) { Text("Remove") }
            }
        }
    }
}

private fun entryMeta(entry: DownloadEntry): String = buildString {
    if (entry.complete) append("Downloaded") else append("${formatBytes(entry.downloaded)} of ${formatBytes(entry.total)}")
    entry.status?.takeIf { !entry.complete && !entry.paused && !entry.controlsBusy() && entry.engineId != null }?.let { status ->
        append("\n${formatSpeed(status.downloadSpeed)}  ${formatPeers(status.numPeers)}")
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
        Box(modifier.clip(shape).background(MaterialTheme.colorScheme.surfaceVariant))
        return
    }
    val pager = rememberPagerState(pageCount = { ready.size })
    HorizontalPager(state = pager, modifier = modifier.clip(shape)) { page ->
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
    val (label, background, foreground) = when {
        entry.controlsBusy() -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        entry.error != null -> Triple("Needs attention", colors.errorContainer, colors.onErrorContainer)
        entry.complete -> Triple("Complete", colors.primaryContainer, colors.onPrimaryContainer)
        entry.paused || entry.engineId == null -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        else -> Triple("Downloading", colors.primaryContainer, colors.onPrimaryContainer)
    }
    Text(label, modifier = Modifier.background(background, RoundedCornerShape(6.dp)).padding(horizontal = 8.dp, vertical = 4.dp), style = MaterialTheme.typography.labelSmall, color = foreground)
}

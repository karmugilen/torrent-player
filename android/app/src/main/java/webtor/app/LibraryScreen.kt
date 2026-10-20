package webtor.app

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(
    state: UiState,
    onAdd: () -> Unit,
    onSettings: () -> Unit,
    onPause: (DownloadEntry) -> Unit,
    onResume: (DownloadEntry) -> Unit,
    onPlay: (DownloadEntry, Int?) -> Unit,
    onOpen: (DownloadEntry) -> Unit,
    onDelete: (DownloadEntry) -> Unit,
    onRetry: (DownloadEntry) -> Unit,
    onMessageShown: () -> Unit,
    onPauseBatch: ((Set<String>) -> Unit)? = null,
    onResumeBatch: ((Set<String>) -> Unit)? = null,
    onDeleteBatch: ((Set<String>, Boolean) -> Unit)? = null,
) {
    val snackbar = remember { SnackbarHostState() }
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf(LibraryFilter.ALL) }
    var sort by rememberSaveable { mutableStateOf(LibrarySort.RECENT) }
    var selectedKeys by rememberSaveable { mutableStateOf(emptySet<String>()) }
    var showBatchDeleteDialog by remember { mutableStateOf(false) }
    var sortMenuOpen by remember { mutableStateOf(false) }

    val inSelectionMode = selectedKeys.isNotEmpty()
    val listState = rememberSaveable(saver = LazyListState.Saver) { LazyListState() }

    LaunchedEffect(state.message) {
        state.message?.let { snackbar.showSnackbar(it); onMessageShown() }
    }

    // Keep selection valid against disappearing/deleted items
    LaunchedEffect(state.library) {
        val currentKeys = state.library.map { it.key }.toSet()
        if (selectedKeys.any { it !in currentKeys }) {
            selectedKeys = selectedKeys.filter { it in currentKeys }.toSet()
        }
    }

    val colors = MaterialTheme.colorScheme
    val filterCounts = remember(state.library, query) {
        val searched = state.library.filterLibrary(query)
        LibraryFilter.entries.associateWith { kind -> searched.count { it.matchesFilter(kind) } }
    }
    val visible by produceState(state.library, state.library, sort, filter, query) {
        value = withContext(Dispatchers.Default) {
            state.library.stableLibrarySort(sort).applyFilter(filter).filterLibrary(query)
        }
    }
    val activityLine = libraryActivityLine(state.library)

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            if (inSelectionMode) {
                TopAppBar(
                    title = {
                        Text(
                            "${selectedKeys.size} selected",
                            style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.Bold),
                        )
                    },
                    navigationIcon = {
                        IconButton(onClick = { selectedKeys = emptySet() }) {
                            Icon(Icons.Default.Close, contentDescription = "Clear selection")
                        }
                    },
                    actions = {
                        IconButton(onClick = {
                            onPauseBatch?.invoke(selectedKeys)
                            selectedKeys = emptySet()
                        }) {
                            Text("Pause", style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold)
                        }
                        IconButton(onClick = {
                            onResumeBatch?.invoke(selectedKeys)
                            selectedKeys = emptySet()
                        }) {
                            Text("Resume", style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold)
                        }
                        IconButton(onClick = { showBatchDeleteDialog = true }) {
                            Icon(Icons.Default.Delete, contentDescription = "Delete selected")
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = colors.primaryContainer,
                        titleContentColor = colors.onPrimaryContainer,
                        actionIconContentColor = colors.onPrimaryContainer,
                        navigationIconContentColor = colors.onPrimaryContainer,
                    ),
                )
            } else {
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
                        titleContentColor = colors.onBackground,
                        actionIconContentColor = colors.onBackground,
                    ),
                )
            }
        },
        floatingActionButton = {
            if (!inSelectionMode) {
                ExtendedFloatingActionButton(
                    onClick = onAdd,
                    icon = { Icon(Icons.Default.Add, contentDescription = null) },
                    text = {
                        Text(
                            "Add download",
                            style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold),
                        )
                    },
                    containerColor = colors.primaryContainer,
                    contentColor = colors.onPrimaryContainer,
                )
            }
        },
    ) { padding ->
        LazyColumn(
            state = listState,
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (!state.engineReady || state.restoring) item { LibraryNotice(state.statusLine) }
            state.error?.takeIf { !state.addSheetOpen }?.let { item { LibraryNotice(it, true) } }
            state.loadWarning?.let { item { LibraryNotice(it, true) } }
            activityLine?.let { line ->
                item {
                    Text(
                        line,
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }

            if (state.library.isNotEmpty()) item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = query,
                        onValueChange = { query = it },
                        modifier = Modifier.fillMaxWidth(),
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

                    // Filters and Sorting controls
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Row(
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            LibraryFilter.entries.forEach { kind ->
                                FilterChip(
                                    selected = filter == kind,
                                    onClick = { filter = kind },
                                    label = {
                                        Text(
                                            "${kind.label()} ${filterCounts[kind] ?: 0}",
                                            style = MaterialTheme.typography.labelSmall,
                                        )
                                    },
                                )
                            }
                        }

                        Spacer(Modifier.width(8.dp))

                        Box {
                            TextButton(onClick = { sortMenuOpen = true }) {
                                Text(
                                    "Sort: ${sort.label()}",
                                    style = MaterialTheme.typography.labelSmall,
                                )
                            }
                            DropdownMenu(
                                expanded = sortMenuOpen,
                                onDismissRequest = { sortMenuOpen = false },
                            ) {
                                LibrarySort.entries.forEach { s ->
                                    DropdownMenuItem(
                                        text = { Text(s.label()) },
                                        onClick = {
                                            sort = s
                                            sortMenuOpen = false
                                        },
                                        leadingIcon = if (sort == s) {
                                            { Icon(Icons.Default.Check, contentDescription = null, modifier = Modifier.size(16.dp)) }
                                        } else null,
                                    )
                                }
                            }
                        }
                    }
                }
            }

            // Distinct empty states
            if (visible.isEmpty()) {
                item {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 72.dp, start = 16.dp, end = 16.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Column(
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            when {
                                state.library.isEmpty() -> {
                                    Text(
                                        text = "No downloads yet",
                                        style = MaterialTheme.typography.headlineSmall,
                                        textAlign = TextAlign.Center,
                                    )
                                    Text(
                                        text = "Add a magnet link or torrent file to get started.",
                                        style = MaterialTheme.typography.bodyMedium,
                                        color = colors.onSurfaceVariant,
                                        textAlign = TextAlign.Center,
                                    )
                                }
                                query.isNotBlank() -> {
                                    Text(
                                        text = "No matching downloads",
                                        style = MaterialTheme.typography.titleMedium,
                                        textAlign = TextAlign.Center,
                                    )
                                    Text(
                                        text = "No items match '$query'. Try another search term.",
                                        style = MaterialTheme.typography.bodyMedium,
                                        color = colors.onSurfaceVariant,
                                        textAlign = TextAlign.Center,
                                    )
                                }
                                else -> {
                                    Text(
                                        text = filter.emptyTitle(),
                                        style = MaterialTheme.typography.titleMedium,
                                        textAlign = TextAlign.Center,
                                    )
                                    Text(
                                        text = "There are no downloads matching this filter.",
                                        style = MaterialTheme.typography.bodyMedium,
                                        color = colors.onSurfaceVariant,
                                        textAlign = TextAlign.Center,
                                    )
                                }
                            }
                        }
                    }
                }
            }

            items(visible, key = { it.key }) { entry ->
                val isSelected = entry.key in selectedKeys
                TorrentRow(
                    entry = entry,
                    isSelected = isSelected,
                    inSelectionMode = inSelectionMode,
                    onToggleSelect = {
                        selectedKeys = if (isSelected) selectedKeys - entry.key else selectedKeys + entry.key
                    },
                    onPause = { onPause(entry) },
                    onResume = { onResume(entry) },
                    onPlay = {
                        val selectedVideos = entry.files.count {
                            it.index in entry.selected && (it.name.isVideoName() || it.path.isVideoName())
                        }
                        val resume = entry.resumeFile()
                        onPlay(entry, if (selectedVideos > 1) resume?.index else null)
                    },
                    onOpen = {
                        if (inSelectionMode) {
                            selectedKeys = if (isSelected) selectedKeys - entry.key else selectedKeys + entry.key
                        } else {
                            onOpen(entry)
                        }
                    },
                    onDelete = { onDelete(entry) },
                    onRetry = { onRetry(entry) },
                )
            }
        }
    }

    if (showBatchDeleteDialog && selectedKeys.isNotEmpty()) {
        AlertDialog(
            onDismissRequest = { showBatchDeleteDialog = false },
            title = { Text("Delete ${selectedKeys.size} downloads?") },
            text = { Text("Permanently erase downloaded files from your device, or remove from the library while keeping files?") },
            confirmButton = {
                TextButton(
                    onClick = {
                        val keys = selectedKeys
                        selectedKeys = emptySet()
                        showBatchDeleteDialog = false
                        onDeleteBatch?.invoke(keys, true)
                    },
                ) {
                    Text("Delete files", color = colors.error)
                }
            },
            dismissButton = {
                Row {
                    TextButton(onClick = { showBatchDeleteDialog = false }) {
                        Text("Cancel")
                    }
                    TextButton(
                        onClick = {
                            val keys = selectedKeys
                            selectedKeys = emptySet()
                            showBatchDeleteDialog = false
                            onDeleteBatch?.invoke(keys, false)
                        },
                    ) {
                        Text("Keep files")
                    }
                }
            },
        )
    }
}

@Composable
private fun LibraryNotice(message: String, error: Boolean = false) = Text(
    message,
    color = if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.tertiary,
    style = MaterialTheme.typography.bodyMedium,
    modifier = Modifier.padding(vertical = 10.dp),
)

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TorrentRow(
    entry: DownloadEntry,
    isSelected: Boolean,
    inSelectionMode: Boolean,
    onToggleSelect: () -> Unit,
    onPause: () -> Unit,
    onResume: () -> Unit,
    onPlay: () -> Unit,
    onOpen: () -> Unit,
    onDelete: () -> Unit,
    onRetry: () -> Unit,
) {
    val busy = entry.controlsBusy()
    val progress by animateFloatAsState(entry.progress, tween(200), label = "downloadProgress")

    ThemedCard(
        modifier = Modifier
            .fillMaxWidth()
            .combinedClickable(
                onClick = onOpen,
                onLongClick = onToggleSelect,
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
                if (inSelectionMode) {
                    Checkbox(
                        checked = isSelected,
                        onCheckedChange = { onToggleSelect() },
                        modifier = Modifier.align(Alignment.CenterVertically),
                    )
                }

                if (entry.files.any { it.index in entry.selected && it.name.isVideoName() }) {
                    EpisodeVideoPreview(
                        entry = entry,
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
            if (!entry.complete && !entry.directPlay) {
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
            if (!inSelectionMode) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    val selectedVideos = entry.files.filter { it.index in entry.selected && (it.name.isVideoName() || it.path.isVideoName()) }
                    val isMultiVideo = selectedVideos.size > 1
                    val resumeIndex = if (isMultiVideo) entry.resumeFile()?.index else null
                    val playsResume = resumeIndex != null
                    val canPlay = entry.canPlay()
                    val isVideo = mediaType(entry) == "Video"
                    val playIndex = resumeIndex ?: pickPlayIndex(entry)

                    if (canPlay || entry.complete) {
                        Button(
                            onClick = when {
                                playsResume -> onPlay
                                isMultiVideo -> onOpen
                                canPlay -> onPlay
                                else -> onOpen
                            },
                            enabled = !busy && (!entry.checking || completedPlayFile(entry, playIndex) != null),
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
                        entry.paused || (entry.engineId == null && entry.lifecycleState != EntryLifecycleState.DOWNLOADING) -> {
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
            contentAlignment = Alignment.Center,
        ) {
            Text(if (!entry.metadataReady) "Waiting for file information" else if (entry.engineId != null && !entry.paused) "Preparing preview…" else "No preview yet",
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        return
    }
    val pager = rememberPagerState(pageCount = { ready.size })
    LaunchedEffect(ready.lastOrNull()) {
        if (entry.directPlay && ready.size > 1) pager.scrollToPage(ready.lastIndex)
    }
    Box(modifier = modifier.clip(shape)) {
        HorizontalPager(
            state = pager,
            modifier = Modifier.fillMaxSize(),
        ) { page ->
            Image(
                ready[page].asImageBitmap(),
                contentDescription = "${entry.title}, preview ${page + 1} of ${ready.size}",
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        }
        if (ready.size > 1) {
            Row(
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = if (large) 8.dp else 4.dp)
                    .background(
                        color = Color.Black.copy(alpha = 0.45f),
                        shape = RoundedCornerShape(100),
                    )
                    .padding(PaddingValues(horizontal = 6.dp, vertical = 3.dp)),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                repeat(ready.size) { index ->
                    val isSelected = pager.currentPage == index
                    val dotWidth by animateDpAsState(
                        targetValue = if (isSelected) (if (large) 14.dp else 10.dp) else 4.dp,
                        animationSpec = spring(stiffness = Spring.StiffnessMediumLow),
                        label = "dotWidth",
                    )
                    Box(
                        modifier = Modifier
                            .size(width = dotWidth, height = 4.dp)
                            .background(
                                color = if (isSelected) Color.White else Color.White.copy(alpha = 0.5f),
                                shape = RoundedCornerShape(2.dp),
                            ),
                    )
                }
            }
        }
    }
}

@Composable
private fun StatusMark(entry: DownloadEntry) {
    val colors = MaterialTheme.colorScheme
    val isPaused = entry.paused || entry.lifecycleState == EntryLifecycleState.PAUSED ||
        (entry.metadataReady && (entry.lifecycleState == EntryLifecycleState.STOPPED || entry.engineId == null))
    val (label, bg, fg) = when {
        entry.error != null -> Triple("Needs attention", colors.errorContainer, colors.onErrorContainer)
        entry.complete -> Triple("Complete", colors.primaryContainer, colors.onPrimaryContainer)
        isPaused -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        entry.controlsBusy() -> Triple(entry.stateLabel(), colors.tertiaryContainer, colors.onTertiaryContainer)
        else -> Triple(entry.stateLabel(), colors.primaryContainer, colors.onPrimaryContainer)
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

package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import webtor.app.R

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AddSheet(
    magnet: String, engineReady: Boolean, error: String?, status: String? = null,
    connecting: Boolean = false, onMagnet: (String) -> Unit, onAdd: () -> Unit,
    onOpenFile: () -> Unit, onDismiss: () -> Unit, draft: PrepareDraft? = null,
    freeBytes: Long = 0, onToggle: ((Int) -> Unit)? = null,
    onSelectAll: (() -> Unit)? = null, onSelectNone: (() -> Unit)? = null,
    onSelectVideos: (() -> Unit)? = null,
    onDownload: (() -> Unit)? = null,
    onDirectPlay: (() -> Unit)? = null, directPlayDefault: Boolean = false,
    destination: String = "Downloads/Webtor",
    existingEntry: DownloadEntry? = null, onOpenExisting: (() -> Unit)? = null,
    sourceFileName: String? = null,
) {
    val busy = draft?.busy == true
    BackHandler(enabled = !busy, onBack = onDismiss)
    val torrent = draft?.torrent
    val ready = torrent?.ready == true
    val files = torrent?.files.orEmpty()
    val selection = draft?.selected.orEmpty()
    val selectedBytes = files.filter { it.index in selection }.sumOf { it.length }
    var query by rememberSaveable(draft?.engineId) { mutableStateOf("") }
    var videosOnly by rememberSaveable(draft?.engineId) { mutableStateOf(false) }
    val visibleFiles = remember(files, query, videosOnly) {
        files.filter { (!videosOnly || it.name.isVideoName()) && (query.isBlank() || it.path.contains(query.trim(), ignoreCase = true) || it.name.contains(query.trim(), ignoreCase = true)) }
    }
    val hasVideo = files.any { it.name.isVideoName() }
    val hiddenByVideosOnly = if (videosOnly) files.count { !it.name.isVideoName() } else 0
    val storageKnown = freeBytes >= 0
    val insufficientSpace = storageKnown && freeBytes < selectedBytes
    // Only a magnet can be saved before its file list arrives.
    val canAddPending = !ready && draft != null && isMagnetLink(draft.source.trim()) && !busy && existingEntry == null && onDownload != null && engineReady
    val canDownload = !busy && (ready && selection.isNotEmpty() && !insufficientSpace || canAddPending) && existingEntry == null && onDownload != null
    Scaffold(
        modifier = Modifier.imePadding(),
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Add download", style = MaterialTheme.typography.titleLarge) },
                actions = {
                    IconButton(onClick = onDismiss, enabled = !busy) {
                        Icon(Icons.Default.Close, contentDescription = "Close")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.surface,
                    titleContentColor = MaterialTheme.colorScheme.onBackground,
                    actionIconContentColor = MaterialTheme.colorScheme.onBackground,
                ),
            )
        },
        bottomBar = {
            if (ready || existingEntry != null || canAddPending) Surface(
                color = MaterialTheme.colorScheme.background,
                tonalElevation = 2.dp,
            ) {
                Column(
                    Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp, vertical = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text("$destination · ${if (storageKnown) "${formatBytes(freeBytes)} free" else "Free space unavailable"}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    when {
                        existingEntry != null -> Text("This torrent is already in your library.", style = MaterialTheme.typography.bodyMedium)
                        insufficientSpace -> Text("Not enough storage to download ${formatBytes(selectedBytes)}.", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                        selection.isEmpty() && ready -> Text("Select at least one file to continue.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        canAddPending -> Text("No peers yet. Save this magnet and keep trying in the background.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (existingEntry != null) {
                        Button(
                            onClick = { onOpenExisting?.invoke() },
                            enabled = !busy && onOpenExisting != null,
                            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
                        ) {
                            Text("Open existing torrent")
                        }
                    } else {
                        if (ready) Text(
                            "Download writes the selected files into the destination folder. Direct Play keeps the video in RAM and does not save the movie.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Button(
                            onClick = { if (directPlayDefault && ready) onDirectPlay?.invoke() else onDownload?.invoke() },
                            enabled = if (directPlayDefault && ready) selection.isNotEmpty() && !busy else canDownload,
                            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
                        ) {
                            Text(if (canAddPending) "Save and wait for peers" else if (directPlayDefault) "Watch now (Direct Play)" else "Save for offline (Download)", maxLines = 1)
                        }
                        if (ready) OutlinedButton(
                            onClick = { if (directPlayDefault) onDownload?.invoke() else onDirectPlay?.invoke() },
                            enabled = !busy && selection.isNotEmpty() && (!directPlayDefault || !insufficientSpace),
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(
                                if (directPlayDefault) "Save for offline (Download)" else "Watch now (Direct Play)",
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
            }
        },
    ) { insets ->
        LazyColumn(
            Modifier.fillMaxSize().padding(insets),
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            item {
                SourceInput(magnet, engineReady, connecting && existingEntry == null, ready, busy, status, onMagnet, onAdd, onOpenFile, existingEntry != null, sourceFileName)
            }
            (error ?: draft?.error)?.let { message -> item {
                Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer), shape = MaterialTheme.shapes.medium) {
                    Text(message, Modifier.padding(14.dp), color = MaterialTheme.colorScheme.onErrorContainer, style = MaterialTheme.typography.bodyMedium)
                }
            } }
            if (ready) {
                item { draft?.let { DraftVideoPreview(it) } }
                item {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(torrent?.name ?: "Choose files", style = MaterialTheme.typography.titleMedium)
                        Text("${selection.size} of ${files.size} files selected · ${formatBytes(selectedBytes)}", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text("Videos and matching subtitles start selected.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        FlowRow(
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                            verticalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            TextButton(onClick = { onSelectAll?.invoke() }, enabled = !busy) { Text("Select all") }
                            TextButton(onClick = { onSelectNone?.invoke() }, enabled = !busy) { Text("Select none") }
                            TextButton(onClick = { onSelectVideos?.invoke() }, enabled = !busy && hasVideo) { Text("Videos") }
                            FilterChip(selected = videosOnly, onClick = { videosOnly = !videosOnly }, label = { Text("Videos only") })
                        }
                        if (hiddenByVideosOnly > 0) Text(
                            if (hiddenByVideosOnly == 1) "Videos only hides 1 file." else "Videos only hides $hiddenByVideosOnly files.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (files.size >= 8 || query.isNotEmpty()) OutlinedTextField(
                            value = query, onValueChange = { query = it }, singleLine = true,
                            modifier = Modifier.fillMaxWidth(), label = { Text("Search files") },
                            leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
                            trailingIcon = if (query.isNotEmpty()) ({
                                IconButton(onClick = { query = "" }) {
                                    Icon(Icons.Default.Clear, contentDescription = "Clear search")
                                }
                            }) else null,
                        )
                    }
                }
                if (visibleFiles.isEmpty()) item { Text("No files match your search or filter.", style = MaterialTheme.typography.bodyMedium) }
                items(visibleFiles, key = { it.index }) { file ->
                    val selected = file.index in selection
                    Column {
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 56.dp)
                                .toggleable(value = selected, enabled = !busy && onToggle != null, role = Role.Checkbox) { onToggle?.invoke(file.index) }
                                .padding(vertical = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(checked = selected, onCheckedChange = null, enabled = !busy)
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(file.path.ifBlank { file.name }, maxLines = 3, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                                Text(
                                    "${fileKindLabel(file.name, file.path)} · ${formatBytes(file.length)}",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                    }
                }
            }
        }
    }
}

@Composable
private fun SourceInput(
    magnet: String, engineReady: Boolean, connecting: Boolean, ready: Boolean, busy: Boolean, status: String?,
    onMagnet: (String) -> Unit, onAdd: () -> Unit, onOpenFile: () -> Unit, alreadyAdded: Boolean = false,
    sourceFileName: String? = null,
) {
    val clipboard = LocalClipboardManager.current
    val focusManager = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    var pastedSourceToHide by remember { mutableStateOf<String?>(null) }
    var pasteHint by remember { mutableStateOf<String?>(null) }
    fun hideInput() {
        keyboard?.hide()
        focusManager.clearFocus()
    }
    fun pasteFromClipboard() {
        val clipText = clipboard.getText()?.text?.trim().orEmpty()
        val link = extractTorrentLink(clipText)
        pasteHint = when {
            clipText.isEmpty() -> "The clipboard is empty. Copy a magnet link or a .torrent link first."
            link == null -> "The clipboard has no magnet link or .torrent link."
            else -> null
        }
        if (link != null) {
            hideInput()
            onMagnet(link)
        }
    }
    LaunchedEffect(pastedSourceToHide) {
        val pasted = pastedSourceToHide ?: return@LaunchedEffect
        // Let the IME finish its paste transaction before changing focus. Doing
        // this synchronously can close the keyboard before Paste is committed.
        delay(250)
        if (magnet == pasted) hideInput()
        pastedSourceToHide = null
    }
    val fromFile = sourceFileName != null && magnet.isEmpty()
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (!ready && !alreadyAdded && !fromFile) Text("Paste a magnet link or a link to a .torrent file, or open a .torrent file from your device.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (fromFile) {
            // A torrent opened from a file has no link to show; name the file instead.
            OutlinedCard(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.medium) {
                Row(
                    Modifier.fillMaxWidth().padding(start = 16.dp, top = 6.dp, bottom = 6.dp, end = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text("Torrent file", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text(sourceFileName.orEmpty(), style = MaterialTheme.typography.bodyLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    IconButton(onClick = { onMagnet("") }, enabled = !busy) {
                        Icon(Icons.Default.Clear, contentDescription = "Remove this torrent file")
                    }
                }
            }
        } else OutlinedTextField(
            value = magnet,
            onValueChange = { value ->
                val fullLinkWasPasted = shouldHideKeyboardAfterPaste(magnet, value)
                pasteHint = null
                onMagnet(value)
                if (fullLinkWasPasted) pastedSourceToHide = value
            },
            enabled = !busy, modifier = Modifier.fillMaxWidth(),
            label = { Text("Magnet link or torrent URL") }, placeholder = { Text("magnet:?xt=urn:btih:…") },
            trailingIcon = {
                if (magnet.isNotEmpty()) {
                    IconButton(onClick = { onMagnet("") }, enabled = !busy) {
                        Icon(Icons.Default.Clear, contentDescription = "Clear link")
                    }
                } else {
                    IconButton(onClick = ::pasteFromClipboard, enabled = !busy) {
                        Icon(
                            painter = painterResource(R.drawable.ic_content_paste),
                            contentDescription = "Paste from clipboard",
                        )
                    }
                }
            },
            minLines = if (ready) 1 else 2, maxLines = 3,
            keyboardOptions = KeyboardOptions(
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Done,
                autoCorrectEnabled = false,
            ),
            keyboardActions = KeyboardActions(onDone = {
                hideInput()
                if (engineReady && looksLikeTorrentSource(magnet) && !connecting && !busy) onAdd()
            }),
        )
        pasteHint?.takeIf { magnet.isEmpty() }?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
        if (!engineReady) Text("Starting download engine…", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (!alreadyAdded && (connecting || status != null && !ready)) Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (connecting) LinearProgressIndicator(Modifier.fillMaxWidth())
            Text(status ?: "Finding torrent metadata…", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        when {
            alreadyAdded || ready || fromFile -> Unit
            // Nothing entered yet: offer the two ways to bring a torrent in.
            magnet.isBlank() -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = ::pasteFromClipboard, enabled = !busy,
                    modifier = Modifier.weight(1f).heightIn(min = 48.dp),
                ) {
                    Icon(painterResource(R.drawable.ic_content_paste), contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Paste link", maxLines = 1)
                }
                OutlinedButton(
                    onClick = onOpenFile, enabled = engineReady && !busy,
                    modifier = Modifier.weight(1f).heightIn(min = 48.dp),
                ) { Text("Open file", maxLines = 1) }
            }
            else -> {
                if (magnet.isNotBlank() && !looksLikeTorrentSource(magnet)) Text(
                    "This is not a magnet link or a link to a .torrent file.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error,
                )
                Button(
                    onClick = onAdd, enabled = engineReady && looksLikeTorrentSource(magnet) && !connecting && !busy,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
                ) { Text(if (connecting) "Reading torrent…" else "Find files") }
                TextButton(onClick = onOpenFile, enabled = engineReady && !busy, modifier = Modifier.fillMaxWidth()) {
                    Text("Open a torrent file instead")
                }
            }
        }
    }
}

private fun fileKindLabel(name: String, path: String): String = when {
    name.isVideoName() || path.isVideoName() -> "Video"
    name.isSubtitleName() || path.isSubtitleName() -> "Subtitle"
    name.isAudioName() || path.isAudioName() -> "Audio"
    else -> "Other"
}

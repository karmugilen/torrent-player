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
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AddSheet(
    magnet: String, engineReady: Boolean, error: String?, status: String? = null,
    connecting: Boolean = false, onMagnet: (String) -> Unit, onAdd: () -> Unit,
    onOpenFile: () -> Unit, onDismiss: () -> Unit, draft: PrepareDraft? = null,
    freeBytes: Long = 0, onToggle: ((Int) -> Unit)? = null,
    onSelectAll: (() -> Unit)? = null, onSelectNone: (() -> Unit)? = null,
    onDownload: (() -> Unit)? = null,
    destination: String = "Downloads/Webtor",
    existingEntry: DownloadEntry? = null, onOpenExisting: (() -> Unit)? = null,
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
    val storageKnown = freeBytes >= 0
    val insufficientSpace = storageKnown && freeBytes < selectedBytes
    val canDownload = ready && selection.isNotEmpty() && !busy && !insufficientSpace && existingEntry == null && onDownload != null
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
                    containerColor = MaterialTheme.colorScheme.background,
                    titleContentColor = MaterialTheme.colorScheme.onBackground,
                    actionIconContentColor = MaterialTheme.colorScheme.onBackground,
                ),
            )
        },
        bottomBar = {
            if (ready || existingEntry != null) Surface(
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
                        selection.isEmpty() -> Text("Select at least one file to continue.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
                        Button(
                            onClick = { onDownload?.invoke() },
                            enabled = canDownload,
                            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
                        ) {
                            Text("Download", maxLines = 1)
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
                SourceInput(magnet, engineReady, connecting && existingEntry == null, ready, busy, status, onMagnet, onAdd, onOpenFile, existingEntry != null)
            }
            (error ?: draft?.error)?.let { message -> item {
                Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer), shape = MaterialTheme.shapes.medium) {
                    Text(message, Modifier.padding(14.dp), color = MaterialTheme.colorScheme.onErrorContainer, style = MaterialTheme.typography.bodyMedium)
                }
            } }
            if (ready) {
                item {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(torrent?.name ?: "Choose files", style = MaterialTheme.typography.titleMedium)
                        Text("${selection.size} of ${files.size} files selected · ${formatBytes(selectedBytes)}", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text("Videos are selected by default when available.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(onClick = { onSelectAll?.invoke() }, enabled = !busy) { Text("Select all") }
                            TextButton(onClick = { onSelectNone?.invoke() }, enabled = !busy) { Text("Select none") }
                            FilterChip(selected = videosOnly, onClick = { videosOnly = !videosOnly }, label = { Text("Videos only") })
                        }
                        OutlinedTextField(
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
                                Text(formatBytes(file.length), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
) {
    val clipboard = LocalClipboardManager.current
    val focusManager = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    var pastedSourceToHide by remember { mutableStateOf<String?>(null) }
    fun hideInput() {
        keyboard?.hide()
        focusManager.clearFocus()
    }
    LaunchedEffect(pastedSourceToHide) {
        val pasted = pastedSourceToHide ?: return@LaunchedEffect
        // Let the IME finish its paste transaction before changing focus. Doing
        // this synchronously can close the keyboard before Paste is committed.
        delay(250)
        if (magnet == pasted) hideInput()
        pastedSourceToHide = null
    }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (!ready && !alreadyAdded) Text("Paste a magnet link or torrent URL, or open a .torrent file from your device.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedTextField(
            value = magnet,
            onValueChange = { value ->
                val fullLinkWasPasted = shouldHideKeyboardAfterPaste(magnet, value)
                onMagnet(value)
                if (fullLinkWasPasted) pastedSourceToHide = value
            },
            enabled = !busy, modifier = Modifier.fillMaxWidth(),
            label = { Text("Magnet link or torrent URL") }, placeholder = { Text("magnet:?xt=urn:btih:…") },
            trailingIcon = {
                if (magnet.isNotEmpty()) {
                    IconButton(onClick = { onMagnet("") }, enabled = !busy) {
                        Icon(Icons.Default.Clear, contentDescription = "Clear magnet link")
                    }
                } else {
                    TextButton(
                        onClick = {
                            val clipText = clipboard.getText()?.text?.trim().orEmpty()
                            if (clipText.isNotEmpty()) {
                                val supported = supportedTorrentLink(clipText) ?: clipText
                                onMagnet(supported)
                            }
                        },
                        enabled = !busy,
                    ) {
                        Text("Paste", style = MaterialTheme.typography.labelMedium)
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
        if (!engineReady) Text("Starting download engine…", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (!alreadyAdded && (connecting || status != null && !ready)) Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (connecting) LinearProgressIndicator(Modifier.fillMaxWidth())
            Text(status ?: "Finding torrent metadata…", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (!ready && !alreadyAdded) Button(
            onClick = onAdd, enabled = engineReady && looksLikeTorrentSource(magnet) && !connecting && !busy,
            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
        ) { Text(if (connecting) "Reading torrent…" else "Find files") }
        OutlinedButton(onClick = onOpenFile, enabled = engineReady && !busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
            Text(if (ready) "Choose another torrent file" else "Open torrent file")
        }
    }
}

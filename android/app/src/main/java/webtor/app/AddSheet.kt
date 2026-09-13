package webtor.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import webtor.core.TorrentFile
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddSheet(
    magnet: String,
    engineReady: Boolean,
    error: String?,
    status: String? = null,
    connecting: Boolean = false,
    onMagnet: (String) -> Unit,
    onAdd: () -> Unit,
    onOpenFile: () -> Unit,
    onDismiss: () -> Unit,
    draft: PrepareDraft? = null,
    freeBytes: Long = 0,
    onToggle: ((Int) -> Unit)? = null,
    onSelectAll: (() -> Unit)? = null,
    onSelectNone: (() -> Unit)? = null,
    onDownload: (() -> Unit)? = null,
) {
    androidx.activity.compose.BackHandler(onBack = onDismiss)
    androidx.compose.material3.Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = { androidx.compose.material3.TopAppBar(title = { Text("Add download") }, actions = { TextButton(onClick = onDismiss) { Text("Close") } }) },
        bottomBar = { Button(onClick = { onDownload?.invoke() }, enabled = draft?.torrent?.ready == true && draft.selected.isNotEmpty() && !draft.busy && freeBytes >= (draft.torrent.files.filter { it.index in draft.selected }.sumOf { it.length }), modifier = Modifier.fillMaxWidth().padding(24.dp)) { Text(if (draft?.busy == true) "Downloading…" else "Download selected") } },
    ) { padding ->
        Column(
            Modifier
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(padding)
                .padding(horizontal = 24.dp)
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            OutlinedTextField(
                value = magnet,
                onValueChange = onMagnet,
                modifier = Modifier.fillMaxWidth(),
                placeholder = { Text("Magnet or URL") },
                singleLine = true,
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = MaterialTheme.colorScheme.primary,
                    unfocusedBorderColor = MaterialTheme.colorScheme.outline,
                    focusedContainerColor = MaterialTheme.colorScheme.surface,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surface,
                ),
            )
            if (connecting || status != null) {
                if (connecting) {
                    LinearProgressIndicator(
                        Modifier.fillMaxWidth().height(2.dp),
                        color = MaterialTheme.colorScheme.primary,
                        trackColor = MaterialTheme.colorScheme.outline,
                    )
                }
                status?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            Button(
                onClick = onAdd,
                enabled = engineReady && magnet.isNotBlank(),
                elevation = ButtonDefaults.buttonElevation(0.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp),
            ) { Text("Add") }
            OutlinedButton(
                onClick = onOpenFile,
                enabled = engineReady,
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp),
            ) { Text("Open torrent file") }
            if (draft?.torrent?.ready == true) {
                val files = draft.torrent.files
                Row { TextButton(onClick = { onSelectAll?.invoke() }) { Text("All") }; TextButton(onClick = { onSelectNone?.invoke() }) { Text("None") } }
                LazyColumn(Modifier.heightIn(max = 360.dp)) { items(files, key = { it.index }) { file ->
                    Row(Modifier.fillMaxWidth().clickable { onToggle?.invoke(file.index) }.padding(vertical = 10.dp)) { Text(if (file.index in draft.selected) "✓" else "□", Modifier.padding(end = 12.dp)); Column { Text(file.path.ifBlank { file.name }); Text(formatBytes(file.length), style = MaterialTheme.typography.bodySmall) } }
                } }
                Text("${draft.selected.size} files · ${formatBytes(files.filter { it.index in draft.selected }.sumOf { it.length })} · Downloads/Webtor · ${formatBytes(freeBytes)} free", style = MaterialTheme.typography.bodySmall)
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }
}

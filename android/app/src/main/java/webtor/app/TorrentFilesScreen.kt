package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TorrentFilesScreen(
    entry: DownloadEntry,
    error: String?,
    onBack: () -> Unit,
    onPlay: (Int) -> Unit,
) {
    BackHandler(onBack = onBack)
    val files = entry.files.filter { it.index in entry.selected }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Files", style = MaterialTheme.typography.headlineSmall) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(horizontal = 24.dp, vertical = 8.dp),
        ) {
            item {
                Column(Modifier.padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(entry.title, style = MaterialTheme.typography.titleLarge)
                    Text(
                        "${files.size} files · ${formatBytes(entry.total)}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    (error ?: entry.error)?.let {
                        Text(it, color = MaterialTheme.colorScheme.error)
                    }
                }
            }
            items(files, key = { it.index }) { file ->
                TorrentFileRow(file, entry.paused, onPlay = { onPlay(file.index) })
            }
        }
    }
}

@Composable
private fun TorrentFileRow(file: SavedFile, paused: Boolean, onPlay: () -> Unit) {
    val complete = file.length == 0L || file.progress >= 1.0
    val progress by animateFloatAsState(file.progress.toFloat(), tween(500), label = "fileProgress")
    Column(
        Modifier.fillMaxWidth().clickable(enabled = file.uri != null, onClick = onPlay).padding(vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(file.name, style = MaterialTheme.typography.titleMedium)
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "${formatBytes((file.length * file.progress).toLong())} / ${formatBytes(file.length)} · " +
                    when {
                        file.uri == null -> "Not downloaded"
                        complete -> "Done"
                        paused -> "Paused"
                        else -> "Downloading"
                    },
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            TextButton(onClick = onPlay, enabled = file.uri != null) { Text("Play") }
        }
        LinearProgressIndicator(
            progress = { progress.coerceIn(0f, 1f) },
            modifier = Modifier.fillMaxWidth().height(2.dp),
            color = MaterialTheme.colorScheme.primary,
            trackColor = MaterialTheme.colorScheme.outline,
        )
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outline)
}

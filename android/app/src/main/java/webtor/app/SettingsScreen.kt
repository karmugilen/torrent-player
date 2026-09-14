package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    state: UiState,
    onBack: () -> Unit,
    onMaxPeers: (Int) -> Unit,
    onMaxPeersCommit: () -> Unit,
    onDarkTheme: (Boolean) -> Unit,
    onCleanup: () -> Unit,
    onClearAll: () -> Unit,
    onStopAll: () -> Unit,
    onStopAllAndExit: () -> Unit,
) {
    BackHandler(onBack = onBack)
    val colors = MaterialTheme.colorScheme

    Scaffold(
        containerColor = colors.background,
        topBar = {
            TopAppBar(
                title = { Text("Settings", style = MaterialTheme.typography.titleLarge) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = colors.background,
                    titleContentColor = colors.onBackground,
                    navigationIconContentColor = colors.onBackground,
                ),
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        text = "Appearance",
                        style = MaterialTheme.typography.titleMedium,
                        color = colors.onBackground,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                    Card(
                        modifier = Modifier.fillMaxWidth(),
                        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        ListItem(
                            headlineContent = { Text("Dark theme", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface) },
                            supportingContent = { Text("Use dark appearance across the app", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant) },
                            trailingContent = {
                                Switch(
                                    checked = state.darkTheme,
                                    onCheckedChange = onDarkTheme,
                                )
                            },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        )
                    }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        text = "Downloads & storage",
                        style = MaterialTheme.typography.titleMedium,
                        color = colors.onBackground,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                    Card(
                        modifier = Modifier.fillMaxWidth(),
                        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        val storageKnown = state.freeBytes >= 0L
                        val totalStorage = (state.managedBytes + state.freeBytes).coerceAtLeast(1L)
                        val storageFraction = if (storageKnown) (state.managedBytes.toFloat() / totalStorage.toFloat()).coerceIn(0f, 1f) else 0f
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(16.dp),
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Text("Location: Downloads/Webtor", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                            if (storageKnown) {
                                LinearProgressIndicator(
                                    progress = { storageFraction },
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .height(6.dp)
                                        .clip(MaterialTheme.shapes.small),
                                    color = colors.primary,
                                    trackColor = colors.surface,
                                )
                            }
                            Text(
                                if (storageKnown) "${formatBytes(state.freeBytes)} free · ${formatBytes(state.managedBytes)} used" else "Free space unavailable",
                                style = MaterialTheme.typography.bodyMedium,
                                color = colors.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        text = "Network & peers",
                        style = MaterialTheme.typography.titleMedium,
                        color = colors.onBackground,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                    Card(
                        modifier = Modifier.fillMaxWidth(),
                        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(horizontal = 16.dp, vertical = 14.dp),
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                    Text("Max connections", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                    Text(
                                        "Peer connection limit per transfer",
                                        style = MaterialTheme.typography.bodyMedium,
                                        color = colors.onSurfaceVariant,
                                    )
                                }
                                Text(
                                    text = "${state.maxPeers}",
                                    style = MaterialTheme.typography.titleMedium.copy(fontWeight = FontWeight.Bold),
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Slider(
                                value = state.maxPeers.toFloat(),
                                onValueChange = { onMaxPeers(it.toInt()) },
                                onValueChangeFinished = onMaxPeersCommit,
                                valueRange = 8f..80f,
                                steps = 17,
                                colors = SliderDefaults.colors(
                                    thumbColor = colors.primary,
                                    activeTrackColor = colors.primary,
                                    inactiveTrackColor = colors.outlineVariant,
                                ),
                                modifier = Modifier.fillMaxWidth(),
                            )
                        }
                    }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        text = "Session & transfers",
                        style = MaterialTheme.typography.titleMedium,
                        color = colors.onBackground,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                    Card(
                        modifier = Modifier.fillMaxWidth(),
                        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        ListItem(
                            headlineContent = { Text("Active transfers", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface) },
                            supportingContent = {
                                Text("Disconnect peers while keeping files", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                            },
                            trailingContent = {
                                OutlinedButton(
                                    onClick = onStopAll,
                                    enabled = state.library.any { it.engineId != null && !it.isDeleting },
                                    modifier = Modifier.heightIn(min = 36.dp),
                                    contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                                ) {
                                    Text("Stop all")
                                }
                            },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        )
                        HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.5f))
                        ListItem(
                            headlineContent = { Text("Shutdown engine", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface) },
                            supportingContent = {
                                Text("Stop transfers and exit app", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                            },
                            trailingContent = {
                                OutlinedButton(
                                    onClick = onStopAllAndExit,
                                    modifier = Modifier.heightIn(min = 36.dp),
                                    contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                                ) {
                                    Text("Shutdown")
                                }
                            },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        )
                    }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        text = "Cleanup & maintenance",
                        style = MaterialTheme.typography.titleMedium,
                        color = colors.onBackground,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                    Card(
                        modifier = Modifier.fillMaxWidth(),
                        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        ListItem(
                            headlineContent = { Text("Temporary cache", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface) },
                            supportingContent = {
                                Text("Free up transient metadata and piece buffers", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                            },
                            trailingContent = {
                                OutlinedButton(
                                    onClick = onCleanup,
                                    modifier = Modifier.heightIn(min = 36.dp),
                                    contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                                ) {
                                    Text("Clear cache")
                                }
                            },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        )
                        HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.5f))
                        ListItem(
                            headlineContent = {
                                Text(
                                    "Delete all downloads",
                                    style = MaterialTheme.typography.bodyLarge,
                                    color = colors.onSurface,
                                )
                            },
                            supportingContent = {
                                Text(
                                    "Permanently remove downloaded files from device",
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = colors.onSurfaceVariant,
                                )
                            },
                            trailingContent = {
                                Button(
                                    onClick = onClearAll,
                                    colors = ButtonDefaults.buttonColors(
                                        containerColor = colors.errorContainer,
                                        contentColor = colors.onErrorContainer,
                                    ),
                                    modifier = Modifier.heightIn(min = 36.dp),
                                    contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                                ) {
                                    Text("Delete downloaded files")
                                }
                            },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        )
                    }
                }
            }
        }
    }
}

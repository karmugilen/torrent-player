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
import androidx.compose.material3.FilterChip
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
import androidx.compose.material3.TextButton
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
    onAppTheme: (AppTheme) -> Unit,
    onReducedEffects: (Boolean) -> Unit,
    onDirectPlayDefault: (Boolean) -> Unit,
    onAntiDpi: (Boolean) -> Unit,
    onCleanup: () -> Unit,
    onClearAll: () -> Unit,
    onStopAll: () -> Unit,
    onStopAllAndExit: () -> Unit,
    onClearNativeCategory: ((String) -> Unit)? = null,
    onPickFolder: () -> Unit = {},
    onClearFolder: () -> Unit = {},
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
                Card(Modifier.fillMaxWidth()) {
                    ListItem(
                        headlineContent = { Text("Prefer Direct Play") },
                        supportingContent = { Text("On: video stays in RAM; only a small startup cache is saved. Off: download selected files for offline use. Both choices remain available when adding a torrent.") },
                        trailingContent = { Switch(checked = state.directPlayDefault, onCheckedChange = onDirectPlayDefault) },
                    )
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        text = "Appearance & Theme",
                        style = MaterialTheme.typography.titleMedium,
                        color = colors.onBackground,
                        modifier = Modifier.padding(start = 4.dp),
                    )
                    Card(
                        modifier = Modifier.fillMaxWidth(),
                        colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.5f)),
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            Text("Theme style", style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold, color = colors.onSurface)
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                listOf(
                                    AppTheme.AERO to "Aero",
                                    AppTheme.ANTIQUITY to "Antiquity",
                                    AppTheme.RETRO to "Retro",
                                ).forEach { (theme, label) ->
                                    val isSelected = state.appTheme == theme
                                    FilterChip(
                                        selected = isSelected,
                                        onClick = { onAppTheme(theme) },
                                        label = { Text(label) },
                                        modifier = Modifier.weight(1f),
                                    )
                                }
                            }
                            HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.4f))
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(modifier = Modifier.weight(1f)) {
                                    Text("Dark appearance", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                    Text("Use dark styling for the active theme", style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                                }
                                Switch(checked = state.darkTheme, onCheckedChange = onDarkTheme)
                            }
                            HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.4f))
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(modifier = Modifier.weight(1f)) {
                                    Text("Reduced effects", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                    Text("Suppress motion, glows, and complex continuous shaders", style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                                }
                                Switch(checked = state.reducedEffects, onCheckedChange = onReducedEffects)
                            }
                        }
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
                    ThemedCard(modifier = Modifier.fillMaxWidth()) {
                        val storageKnown = state.freeBytes >= 0L
                        val totalStorage = (state.managedBytes + state.freeBytes).coerceAtLeast(1L)
                        val storageFraction = if (storageKnown) (state.managedBytes.toFloat() / totalStorage.toFloat()).coerceIn(0f, 1f) else 0f
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(16.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Text(
                                "Location: ${state.storageFolderName}",
                                style = MaterialTheme.typography.bodyLarge,
                                fontWeight = FontWeight.SemiBold,
                                color = colors.onSurface,
                            )
                            if (state.storageAccessWarning != null) {
                                Card(
                                    colors = CardDefaults.cardColors(containerColor = colors.errorContainer),
                                    modifier = Modifier.fillMaxWidth(),
                                ) {
                                    Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                        Text(
                                            state.storageAccessWarning,
                                            style = MaterialTheme.typography.bodySmall,
                                            color = colors.onErrorContainer,
                                        )
                                        OutlinedButton(
                                            onClick = onPickFolder,
                                            colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.onErrorContainer),
                                        ) {
                                            Text("Reselect folder to restore access")
                                        }
                                    }
                                }
                            }
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
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Button(onClick = onPickFolder) {
                                    Text("Change folder")
                                }
                                if (state.hasCustomFolder) {
                                    OutlinedButton(onClick = onClearFolder) {
                                        Text("Reset to default")
                                    }
                                }
                            }
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
                            HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.4f))
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                    Text("Anti-DPI (experimental)", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                    Text(
                                        "Adaptive connection routing to resist SNI/packet inspection. Experimental feature for your network trials.",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = colors.onSurfaceVariant,
                                    )
                                }
                                Switch(
                                    checked = state.antiDpiEnabled,
                                    onCheckedChange = onAntiDpi,
                                )
                            }
                            if (state.antiDpiEnabled) {
                                val status = state.bypassStatus
                                if (status == null) {
                                    Text(
                                        if (state.engineReady) "Connecting to backend…" else "Engine starting…",
                                        style = MaterialTheme.typography.labelSmall,
                                        color = colors.secondary,
                                    )
                                } else if (status.error != null) {
                                    Text(
                                        "Backend error: ${status.error}",
                                        style = MaterialTheme.typography.labelSmall,
                                        color = colors.error,
                                    )
                                } else {
                                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                        Text(
                                            if (status.active) "Active (mode: ${status.mode})" else "Inactive (mode: ${status.mode})",
                                            style = MaterialTheme.typography.labelMedium,
                                            fontWeight = FontWeight.Bold,
                                            color = if (status.active) colors.primary else colors.error,
                                        )
                                    }
                                    if (status.coverage.isNotEmpty()) {
                                        Text(
                                            "Adapted paths: ${status.coverage.joinToString(", ")}",
                                            style = MaterialTheme.typography.bodySmall,
                                            color = colors.onSurfaceVariant,
                                        )
                                    } else if (status.active) {
                                        Text(
                                            "Adapted paths: active stream routing",
                                            style = MaterialTheme.typography.bodySmall,
                                            color = colors.onSurfaceVariant,
                                        )
                                    }
                                }
                                Text(
                                    "Note: Active indicates the local backend is enabled; test with your ISP to observe actual swarm connectivity.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.outline,
                                )
                            }
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
                            headlineContent = {
                                Text("Temporary cache · ${formatBytes(state.cacheBytes)}", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                            },
                            supportingContent = {
                                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                    Text("Free up thumbnails, temporary imports, and engine caches", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                                    state.nativeCacheSummary?.let { summary ->
                                        if (summary.categories.isNotEmpty()) {
                                            Text(
                                                "Engine categories: " + summary.categories.joinToString(" · ") { "${it.id}: ${formatBytes(it.bytes)}" },
                                                style = MaterialTheme.typography.labelSmall,
                                                color = colors.outline,
                                            )
                                        }
                                    }
                                }
                            },
                            trailingContent = {
                                OutlinedButton(
                                    onClick = onCleanup,
                                    modifier = Modifier.heightIn(min = 36.dp),
                                    contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                                ) {
                                    Text("Clear all cache")
                                }
                            },
                            colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        )
                        state.nativeCacheSummary?.categories?.filter { it.reclaimableBytes > 0 }?.forEach { cat ->
                            HorizontalDivider(color = colors.outlineVariant.copy(alpha = 0.3f))
                            ListItem(
                                headlineContent = { Text("Reclaim ${cat.id} (${formatBytes(cat.reclaimableBytes)})", style = MaterialTheme.typography.bodyMedium) },
                                trailingContent = {
                                    if (onClearNativeCategory != null) {
                                        TextButton(onClick = { onClearNativeCategory(cat.id) }) {
                                            Text("Clear")
                                        }
                                    }
                                },
                                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                            )
                        }
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

package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
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
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Slider
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
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
    onDynamicColors: (Boolean) -> Unit = {},
    onSeeding: (Boolean) -> Unit = {},
    onSeedRatio: (Int) -> Unit = {},
    onSeedTime: (Int) -> Unit = {},
    onSeedUpload: (Int) -> Unit = {},
    onSeedWiFi: (Boolean) -> Unit = {},
    onSeedCharging: (Boolean) -> Unit = {},
    onRamCache: (Int) -> Unit = {},
    onAutoRamCache: () -> Unit = {},
    onPlaybackMethod: (PlaybackMethod) -> Unit = {},
    onPlayBoost: (Boolean) -> Unit = {},
    onAdaptiveBuffer: (Boolean) -> Unit = {},
    onFileOnlyPlayback: (Boolean) -> Unit = {},
    onNetworkPolicy: (TransferNetworkPolicy) -> Unit = {},
    onDownloadRateKiB: (Int) -> Unit = {},
    onFastPeerSearch: (Boolean) -> Unit = {},
    onPauseOnLowBattery: (Boolean) -> Unit = {},
    onPlayNext: (Boolean) -> Unit = {},
    onPreloadStart: (Boolean) -> Unit = {},
) {
    BackHandler(onBack = onBack)
    val colors = MaterialTheme.colorScheme

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Settings", style = MaterialTheme.typography.titleLarge) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = colors.surface,
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
                SettingsSection("Playback") {
                    ListItem(
                        headlineContent = { Text("Prefer Direct Play") },
                        supportingContent = {
                            Text("On: video stays in RAM. A small startup cache may be saved, not the full file. Off: selected files are saved for offline play. Both choices stay available when you add a torrent.")
                        },
                        trailingContent = {
                            Switch(checked = state.directPlayDefault, onCheckedChange = onDirectPlayDefault)
                        },
                        colors = settingsListColors(),
                    )
                }
            }
            item {
                SettingsSection("Appearance") {
                    ListItem(
                        headlineContent = { Text("Dynamic Material colors") },
                        supportingContent = {
                            Text("On Android 12 and newer, colors follow the wallpaper. Older versions, and this switch off, use the palette below.")
                        },
                        trailingContent = {
                            Switch(checked = state.dynamicColors, onCheckedChange = onDynamicColors)
                        },
                        colors = settingsListColors(),
                    )
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            "Color palette",
                            style = MaterialTheme.typography.bodyLarge,
                            fontWeight = FontWeight.SemiBold,
                            color = colors.onSurface,
                        )
                        Text(
                            "Used when dynamic colors are off.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            listOf(
                                AppTheme.AERO to "Aero",
                                AppTheme.ANTIQUITY to "Antiquity",
                                AppTheme.RETRO to "Retro",
                            ).forEach { (theme, label) ->
                                FilterChip(
                                    selected = state.appTheme == theme && !state.dynamicColors,
                                    enabled = !state.dynamicColors,
                                    onClick = { onAppTheme(theme) },
                                    label = { Text(label) },
                                    modifier = Modifier.weight(1f),
                                )
                            }
                        }
                    }
                    SectionDivider()
                    SwitchRow(
                        title = "Dark appearance",
                        supporting = "Use the dark variant of the current colors.",
                        checked = state.darkTheme,
                        onCheckedChange = onDarkTheme,
                    )
                    SectionDivider()
                    SwitchRow(
                        title = "Reduce motion",
                        supporting = "Reduce nonessential connection and interface animations.",
                        checked = state.reducedEffects,
                        onCheckedChange = onReducedEffects,
                    )
                }
            }
            item {
                SettingsSection("Storage") {
                    val storageKnown = state.freeBytes >= 0L
                    val totalStorage = (state.managedBytes + state.freeBytes).coerceAtLeast(1L)
                    val storageFraction = if (storageKnown) {
                        (state.managedBytes.toFloat() / totalStorage.toFloat()).coerceIn(0f, 1f)
                    } else {
                        0f
                    }
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(
                            "Saved downloads go in this folder. Direct Play keeps the video in RAM and does not write the full file here.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
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
                                Column(
                                    modifier = Modifier.padding(12.dp),
                                    verticalArrangement = Arrangement.spacedBy(6.dp),
                                ) {
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
                            if (storageKnown) {
                                "${formatBytes(state.freeBytes)} free · ${formatBytes(state.managedBytes)} used"
                            } else {
                                "Free space unavailable"
                            },
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
            item {
                SettingsSection("Connections") {
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Max connections", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "Peer connection limit per transfer.",
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
                            valueRange = 8f..500f,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        SectionDivider()
                        Text(
                            "RAM cache · ${if (state.ramCacheAuto) "Auto · " else ""}${state.ramCacheMiB} MiB",
                            style = MaterialTheme.typography.bodyLarge,
                        )
                        Text(
                            if (state.ramCacheAuto) {
                                "Auto chooses a limit from available memory and active transfers. Used for downloads and Direct Play."
                            } else {
                                "Cache limit per transfer for downloads and Direct Play. Total memory use grows with active transfers."
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf(0, 64, 128, 256, 512, 1024).forEach { size ->
                                FilterChip(
                                    selected = if (size == 0) state.ramCacheAuto else !state.ramCacheAuto && state.ramCacheMiB == size,
                                    onClick = { if (size == 0) onAutoRamCache() else onRamCache(size) },
                                    label = { Text(if (size == 0) "Auto" else "$size MiB") },
                                )
                            }
                        }
                    }
                }
            }
            item {
                SettingsSection("Seeding") {
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Seed completed downloads", style = MaterialTheme.typography.bodyLarge)
                                Text(
                                    "Shares verified saved pieces after completion. Pauses during playback.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.seedingEnabled, onCheckedChange = onSeeding)
                        }
                        if (state.seedingEnabled) {
                            Text(
                                "Stops at the share ratio or the active seeding time, whichever comes first.",
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.onSurfaceVariant,
                            )
                            Text("Share ratio", style = MaterialTheme.typography.labelLarge)
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                listOf(500, 1000, 2000).forEach { ratio ->
                                    FilterChip(
                                        selected = state.seedRatioMilli == ratio,
                                        onClick = { onSeedRatio(ratio) },
                                        label = { Text("${ratio / 1000.0}×") },
                                    )
                                }
                            }
                            Text("Active seeding time per torrent", style = MaterialTheme.typography.labelLarge)
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                listOf(15, 60, 180).forEach { minutes ->
                                    FilterChip(
                                        selected = state.seedTimeMinutes == minutes,
                                        onClick = { onSeedTime(minutes) },
                                        label = { Text("$minutes min") },
                                    )
                                }
                            }
                            Text("Upload cap", style = MaterialTheme.typography.labelLarge)
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                listOf(128, 256, 512).forEach { speed ->
                                    FilterChip(
                                        selected = state.seedUploadKiB == speed,
                                        onClick = { onSeedUpload(speed) },
                                        label = { Text("$speed KiB/s") },
                                    )
                                }
                            }
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Text("Seed on Wi-Fi only", Modifier.weight(1f))
                                Switch(checked = state.seedWiFiOnly, onCheckedChange = onSeedWiFi)
                            }
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Text("Seed while charging only", Modifier.weight(1f))
                                Switch(checked = state.seedChargingOnly, onCheckedChange = onSeedCharging)
                            }
                            Text(
                                "Upload totals and ratio survive restarts. The peer connection limit is not changed here.",
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
            item {
                SettingsSection("Experimental", outlined = true) {
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text("Playback method", style = MaterialTheme.typography.titleMedium)
                        Text(
                            "Completed downloads still open from the saved file. This choice is for incomplete video and Direct Play.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        Column(Modifier.selectableGroup()) {
                            PlaybackMethod.entries.forEach { method ->
                                val selected = state.playbackMethod == method
                                Row(
                                    modifier = Modifier.fillMaxWidth()
                                        .selectable(
                                            selected = selected,
                                            role = Role.RadioButton,
                                            onClick = { onPlaybackMethod(method) },
                                        )
                                        .padding(vertical = 8.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                                ) {
                                    RadioButton(selected = selected, onClick = null)
                                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                        Text(
                                            if (method == PlaybackMethod.CONTENT_URI) "Standard" else "HTTP stream (experimental)",
                                            style = MaterialTheme.typography.bodyLarge,
                                        )
                                        Text(
                                            if (method == PlaybackMethod.CONTENT_URI) {
                                                "Opens the saved or streaming content URI."
                                            } else {
                                                "Experimental loopback streaming for comparison."
                                            },
                                            style = MaterialTheme.typography.bodySmall,
                                            color = colors.onSurfaceVariant,
                                        )
                                    }
                                }
                            }
                        }
                    }
                    SectionDivider()
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Anti-DPI (experimental)", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "Experimental. Splits the start of some outgoing TCP writes to peers and HTTP or WebSocket trackers. UDP, uTP, and WebRTC are not changed. Not anonymity, and not a promise that a connection succeeds.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.antiDpiEnabled, onCheckedChange = onAntiDpi)
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
                                Text(
                                    if (status.active) "Active (mode: ${status.mode})" else "Inactive (mode: ${status.mode})",
                                    style = MaterialTheme.typography.labelMedium,
                                    fontWeight = FontWeight.Bold,
                                    color = if (status.active) colors.primary else colors.error,
                                )
                                if (status.coverage.isNotEmpty()) {
                                    Text(
                                        "Adapted paths: ${status.coverage.joinToString(", ")}",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = colors.onSurfaceVariant,
                                    )
                                } else if (status.active) {
                                    Text(
                                        "No adapted paths reported.",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = colors.onSurfaceVariant,
                                    )
                                }
                            }
                            Text(
                                "Active means the adapter in this app is running. It does not mean a peer or tracker accepted the connection, and it does not hide your network address.",
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.outline,
                            )
                        }
                    }
                    SectionDivider()
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            "Optional controls for playback and transfers. Some can use more data and battery.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        SectionDivider()
                        Text("More peers", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                        Text(
                            "Raises the same connection limit as Connections. More peers can start a transfer faster and use more battery.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf(100, 200, 500).forEach { peers ->
                                FilterChip(
                                    selected = state.maxPeers == peers,
                                    onClick = {
                                        onMaxPeers(peers)
                                        onMaxPeersCommit()
                                    },
                                    label = { Text("$peers") },
                                )
                            }
                        }
                        SectionDivider()
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Instant play boost", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "Asks the engine for a larger startup buffer when you press Play, so video can begin sooner. It uses more network and memory.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.playBoost, onCheckedChange = onPlayBoost)
                        }
                        SectionDivider()
                        SwitchRow(
                            title = "Adaptive buffer",
                            supporting = "Experimental. Adds look-ahead when read demand varies and reduces it when demand settles. Keeps the same RAM limit. Applies next time you press Play.",
                            checked = state.adaptiveBuffer,
                            onCheckedChange = onAdaptiveBuffer,
                        )
                        SectionDivider()
                        SwitchRow(
                            title = "Play this file only",
                            supporting = "While playing a saved torrent, pauses downloads of its other selected files. They resume when you return from the player. Direct Play already streams only the playing file.",
                            checked = state.fileOnlyPlayback,
                            onCheckedChange = onFileOnlyPlayback,
                        )
                        SectionDivider()
                        Text("Transfer network", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                        Text(
                            "Pauses torrent payload until the network matches. Metadata can still be fetched. Any is the normal choice.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf(
                                TransferNetworkPolicy.ANY to "Any",
                                TransferNetworkPolicy.WIFI_ONLY to "Wi-Fi",
                                TransferNetworkPolicy.UNMETERED to "Unmetered",
                            ).forEach { (policy, label) ->
                                FilterChip(
                                    selected = state.networkPolicy == policy,
                                    onClick = { onNetworkPolicy(policy) },
                                    label = { Text(label) },
                                )
                            }
                        }
                        SectionDivider()
                        Text("Download cap", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                        Text(
                            "Limits download speed only. Seeding upload is unchanged. Unlimited is the normal choice.",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            listOf(
                                0 to "Unlimited",
                                1024 to "1 MB/s",
                                4096 to "4 MB/s",
                                8192 to "8 MB/s",
                            ).forEach { (rate, label) ->
                                FilterChip(
                                    selected = state.downloadRateKiB == rate,
                                    onClick = { onDownloadRateKiB(rate) },
                                    label = { Text(label) },
                                )
                            }
                        }
                        SectionDivider()
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Faster peer search", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "Looks for peers faster than the normal dial rate. It can help a slow swarm and can make a busy network less stable.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.fastPeerSearch, onCheckedChange = onFastPeerSearch)
                        }
                        SectionDivider()
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Preload the start", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "Like TorrServer preload. Waits up to 8 seconds for the first 2 MB before opening the player, so playback is less likely to stall. Play still opens if that data is not ready.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.preloadStart, onCheckedChange = onPreloadStart)
                        }
                        SectionDivider()
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Play next video", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "When a video ends, starts the next selected video in the same torrent.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.playNext, onCheckedChange = onPlayNext)
                        }
                        SectionDivider()
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Pause on low battery", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    if (state.batteryPaused) {
                                        "Paused now. Battery is under 15% and the phone is not charging."
                                    } else {
                                        "Pauses torrent data under 15% battery unless the phone is charging. Playback pauses too."
                                    },
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Switch(checked = state.pauseOnLowBattery, onCheckedChange = onPauseOnLowBattery)
                        }
                        SectionDivider()
                        Text("Incoming port", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                        Text(
                            if (state.listenPort > 0) {
                                "Listening on ${state.listenPort}. The engine already asks the router to forward this port."
                            } else {
                                "The listen port is not ready yet. Incoming peers and port forwarding stay on."
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                    }
                }
            }
            item {
                SettingsSection("Maintenance") {
                    ListItem(
                        headlineContent = {
                            Text("Stop transfers", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                        },
                        supportingContent = {
                            Text(
                                "Disconnects peers and keeps saved files.",
                                style = MaterialTheme.typography.bodyMedium,
                                color = colors.onSurfaceVariant,
                            )
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
                        colors = settingsListColors(),
                    )
                    SectionDivider()
                    ListItem(
                        headlineContent = {
                            Text("Shut down", style = MaterialTheme.typography.bodyLarge, color = colors.error)
                        },
                        supportingContent = {
                            Text(
                                "Stops transfers and exits the app. Saved downloads are not deleted.",
                                style = MaterialTheme.typography.bodyMedium,
                                color = colors.onSurfaceVariant,
                            )
                        },
                        trailingContent = {
                            DangerOutlineButton("Shutdown", onStopAllAndExit)
                        },
                        colors = settingsListColors(),
                    )
                    SectionDivider()
                    ListItem(
                        headlineContent = {
                            Text(
                                "Temporary cache · ${formatBytes(state.cacheBytes)}",
                                style = MaterialTheme.typography.bodyLarge,
                                color = colors.error,
                            )
                        },
                        supportingContent = {
                            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Text(
                                    "Thumbnails, temporary imports, and engine caches. Does not delete saved downloads.",
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = colors.onSurfaceVariant,
                                )
                                state.nativeCacheSummary?.let { summary ->
                                    if (summary.categories.isNotEmpty()) {
                                        Text(
                                            "Engine categories: " + summary.categories.joinToString(" · ") {
                                                "${cacheCategoryLabel(it.id)}: ${formatBytes(it.bytes)}"
                                            },
                                            style = MaterialTheme.typography.labelSmall,
                                            color = colors.outline,
                                        )
                                    }
                                }
                            }
                        },
                        trailingContent = {
                            DangerOutlineButton("Clear all cache", onCleanup)
                        },
                        colors = settingsListColors(),
                    )
                    state.nativeCacheSummary?.categories?.filter { it.reclaimableBytes > 0 }?.forEach { cat ->
                        SectionDivider()
                        ListItem(
                            headlineContent = {
                                Text(
                                    "Reclaim ${cacheCategoryLabel(cat.id)} (${formatBytes(cat.reclaimableBytes)})",
                                    style = MaterialTheme.typography.bodyMedium,
                                )
                            },
                            trailingContent = {
                                if (onClearNativeCategory != null) {
                                    TextButton(
                                        onClick = { onClearNativeCategory(cat.id) },
                                        colors = ButtonDefaults.textButtonColors(contentColor = colors.error),
                                    ) {
                                        Text("Clear")
                                    }
                                }
                            },
                            colors = settingsListColors(),
                        )
                    }
                    SectionDivider()
                    ListItem(
                        headlineContent = {
                            Text(
                                "Delete all downloads",
                                style = MaterialTheme.typography.bodyLarge,
                                color = colors.error,
                            )
                        },
                        supportingContent = {
                            Text(
                                "Permanently removes saved download files. Library rows stay. Direct Play rows are left as they are.",
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
                        colors = settingsListColors(),
                    )
                }
            }
        }
    }
}

@Composable
private fun SettingsSection(title: String, outlined: Boolean = false, content: @Composable ColumnScope.() -> Unit) {
    Column(
        modifier = if (outlined) Modifier.fillMaxWidth()
            .border(1.dp, MaterialTheme.colorScheme.outline, MaterialTheme.shapes.medium)
            .padding(12.dp) else Modifier,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = title,
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.onBackground,
            modifier = Modifier.padding(start = 4.dp),
        )
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
            shape = MaterialTheme.shapes.medium,
        ) {
            content()
        }
    }
}

@Composable
private fun SectionDivider() {
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.4f))
}

@Composable
private fun settingsListColors() = ListItemDefaults.colors(containerColor = Color.Transparent)

@Composable
private fun SwitchRow(
    title: String,
    supporting: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
            Text(supporting, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}

@Composable
private fun DangerOutlineButton(text: String, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    OutlinedButton(
        onClick = onClick,
        modifier = Modifier.heightIn(min = 36.dp),
        contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = scheme.error),
        border = BorderStroke(1.dp, scheme.error),
    ) {
        Text(text)
    }
}

private fun cacheCategoryLabel(id: String): String = when (id) {
    "startup" -> "startup cache"
    "resume" -> "resume data"
    "peers" -> "peer cache"
    "dht" -> "DHT contacts"
    else -> id
}

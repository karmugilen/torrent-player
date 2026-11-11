package webtor.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlin.math.roundToInt
import androidx.compose.foundation.clickable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
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
    onIdleDropMinutes: (Int) -> Unit = {},
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
    onNetworkPolicy: (TransferNetworkPolicy) -> Unit = {},
    onDownloadRateKiB: (Int) -> Unit = {},
    onPauseOnLowBattery: (Boolean) -> Unit = {},
) {
    BackHandler(onBack = onBack)
    val colors = MaterialTheme.colorScheme

    Scaffold(
        containerColor = colors.background,
        topBar = {
            TopAppBar(
                title = { Text("Settings", style = MaterialTheme.typography.titleLarge.copy(fontWeight = FontWeight.Bold)) },
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
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            item(key = "downloads") {
                SettingsGroup("Downloads") {
                    val storageKnown = state.freeBytes >= 0L
                    val totalStorage = (state.managedBytes + state.freeBytes).coerceAtLeast(1L)
                    val storageFraction = if (storageKnown) {
                        (state.managedBytes.toFloat() / totalStorage.toFloat()).coerceIn(0f, 1f)
                    } else 0f
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Save folder", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(state.storageFolderName, style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
                            }
                            Spacer(Modifier.width(12.dp))
                            OutlinedButton(onClick = onPickFolder, contentPadding = CompactButtonPadding) { Text("Change") }
                        }
                        if (storageKnown) {
                            LinearProgressIndicator(
                                progress = { storageFraction },
                                modifier = Modifier.fillMaxWidth().height(6.dp).clip(MaterialTheme.shapes.small),
                                color = colors.primary,
                                trackColor = colors.surfaceVariant,
                            )
                        }
                        Text(
                            if (storageKnown) "${formatBytes(state.managedBytes)} used by downloads · ${formatBytes(state.freeBytes)} free"
                            else "Free space unavailable",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.onSurfaceVariant,
                        )
                        if (state.storageAccessWarning != null) {
                            Card(
                                colors = CardDefaults.cardColors(containerColor = colors.errorContainer),
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                    Text(state.storageAccessWarning, style = MaterialTheme.typography.bodySmall, color = colors.onErrorContainer)
                                    OutlinedButton(
                                        onClick = onPickFolder,
                                        colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.onErrorContainer),
                                    ) { Text("Choose the folder again") }
                                }
                            }
                        }
                        if (state.hasCustomFolder) {
                            TextButton(onClick = onClearFolder, contentPadding = PaddingValues(0.dp)) { Text("Use Downloads/Webtor again") }
                        }
                    }
                    GroupDivider()
                    ChoiceSetting(
                        title = "When adding a torrent",
                        supporting = if (state.directPlayDefault) {
                            "Direct Play streams the video from RAM and saves only what makes it open fast."
                        } else {
                            "Download saves the selected files for offline play."
                        } + " Both stay available each time.",
                        options = listOf(false to "Download", true to "Direct Play"),
                        selected = state.directPlayDefault,
                        onSelect = onDirectPlayDefault,
                    )
                }
            }
            item(key = "playback") {
                SettingsGroup("Playback") {
                    ChoiceSetting(
                        title = "Stop idle Direct Play",
                        supporting = "Frees memory when the player is closed and nothing has played for this long. Play starts it again.",
                        options = listOf(0 to "Off", 5 to "5 min", 15 to "15 min", 30 to "30 min"),
                        selected = state.idleDropMinutes,
                        onSelect = onIdleDropMinutes,
                    )
                    GroupDivider()
                    ChoiceSetting(
                        title = "How players get the video",
                        supporting = if (state.playbackMethod == PlaybackMethod.CONTENT_URI) {
                            "File proxy: the older way, through Android's file access. Try it if a player cannot open the stream."
                        } else {
                            "Stream: the player reads from the engine on this phone. Fastest seeking."
                        } + " Finished downloads always open from the saved file.",
                        options = listOf(PlaybackMethod.HTTP to "Stream", PlaybackMethod.CONTENT_URI to "File proxy"),
                        selected = state.playbackMethod,
                        onSelect = onPlaybackMethod,
                    )
                }
            }
            item(key = "network") {
                SettingsGroup("Network") {
                    ChoiceSetting(
                        title = "Transfer on",
                        supporting = state.networkReason?.takeIf { !state.networkAllowed }
                            ?: "Torrent data waits until the network matches.",
                        options = listOf(
                            TransferNetworkPolicy.ANY to "Any network",
                            TransferNetworkPolicy.WIFI_ONLY to "Wi-Fi",
                            TransferNetworkPolicy.UNMETERED to "Unmetered",
                        ),
                        selected = state.networkPolicy,
                        onSelect = onNetworkPolicy,
                    )
                    GroupDivider()
                    ChoiceSetting(
                        title = "Download speed limit",
                        supporting = null,
                        options = listOf(0 to "None", 1024 to "1 MB/s", 4096 to "4 MB/s", 8192 to "8 MB/s"),
                        selected = state.downloadRateKiB,
                        onSelect = onDownloadRateKiB,
                    )
                    GroupDivider()
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Peers per torrent", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    "More peers can be faster and use more battery.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            ValueBadge("${state.maxPeers}")
                        }
                        Slider(
                            value = state.maxPeers.toFloat(),
                            onValueChange = { onMaxPeers(it.toInt()) },
                            onValueChangeFinished = onMaxPeersCommit,
                            valueRange = 8f..500f,
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                    GroupDivider()
                    SwitchSetting(
                        title = "Anti-DPI",
                        supporting = when {
                            !state.antiDpiEnabled ->
                                "Experimental, for networks that block torrents. Not working in this version yet."
                            state.bypassStatus?.error != null -> "Not running: ${state.bypassStatus.error}."
                            state.bypassStatus?.active == true -> "On for new TCP peer connections. It does not hide your address."
                            else -> "Turning on…"
                        },
                        checked = state.antiDpiEnabled,
                        onCheckedChange = onAntiDpi,
                    )
                    GroupDivider()
                    SettingRow(
                        title = "Incoming port",
                        supporting = if (state.listenPort > 0) "${state.listenPort}, forwarded by the router when it allows it."
                        else "Not ready yet.",
                    )
                }
            }
            item(key = "seeding") {
                SettingsGroup("Seeding") {
                    SwitchSetting(
                        title = "Seed finished downloads",
                        supporting = "Shares what you downloaded until a limit below is reached. Pauses while a video plays.",
                        checked = state.seedingEnabled,
                        onCheckedChange = onSeeding,
                    )
                    if (state.seedingEnabled) {
                        GroupDivider()
                        ChoiceSetting(
                            title = "Stop at share ratio",
                            supporting = null,
                            options = listOf(500 to "0.5×", 1000 to "1×", 2000 to "2×"),
                            selected = state.seedRatioMilli,
                            onSelect = onSeedRatio,
                        )
                        GroupDivider()
                        ChoiceSetting(
                            title = "Or after seeding for",
                            supporting = null,
                            options = listOf(15 to "15 min", 60 to "1 hour", 180 to "3 hours"),
                            selected = state.seedTimeMinutes,
                            onSelect = onSeedTime,
                        )
                        GroupDivider()
                        ChoiceSetting(
                            title = "Upload speed limit",
                            supporting = null,
                            options = listOf(128 to "128 KiB/s", 256 to "256 KiB/s", 512 to "512 KiB/s"),
                            selected = state.seedUploadKiB,
                            onSelect = onSeedUpload,
                        )
                        GroupDivider()
                        SwitchSetting("Only on Wi-Fi", null, state.seedWiFiOnly, onSeedWiFi)
                        GroupDivider()
                        SwitchSetting("Only while charging", null, state.seedChargingOnly, onSeedCharging)
                    }
                }
            }
            item(key = "device") {
                SettingsGroup("Battery and memory") {
                    SwitchSetting(
                        title = "Pause on low battery",
                        supporting = if (state.batteryPaused) "Paused now: battery is under 15% and the phone is not charging."
                        else "Stops torrent data and playback under 15% battery unless the phone is charging.",
                        checked = state.pauseOnLowBattery,
                        onCheckedChange = onPauseOnLowBattery,
                    )
                    GroupDivider()
                    // The slider moves freely; the engine applies the size when it is released.
                    var ramStep by remember(state.ramCacheMiB, state.ramCacheAuto) {
                        mutableFloatStateOf(ramCacheStepIndex(state.ramCacheMiB).toFloat())
                    }
                    val ramDraft = RAM_CACHE_STEPS_MIB[ramStep.roundToInt().coerceIn(RAM_CACHE_STEPS_MIB.indices)]
                    Column(
                        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text("Memory per torrent", style = MaterialTheme.typography.bodyLarge, color = colors.onSurface)
                                Text(
                                    if (state.ramCacheAuto) "Auto picks a size from free memory. Move the slider to set your own."
                                    else "Used for downloads and Direct Play. Smaller keeps seeking quick.",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.onSurfaceVariant,
                                )
                            }
                            Spacer(Modifier.width(12.dp))
                            ValueBadge(formatRamCacheMiB(ramDraft))
                        }
                        Slider(
                            value = ramStep,
                            onValueChange = { ramStep = it },
                            onValueChangeFinished = { onRamCache(RAM_CACHE_STEPS_MIB[ramStep.roundToInt().coerceIn(RAM_CACHE_STEPS_MIB.indices)]) },
                            valueRange = 0f..RAM_CACHE_STEPS_MIB.lastIndex.toFloat(),
                            steps = RAM_CACHE_STEPS_MIB.size - 2,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        FilterChip(selected = state.ramCacheAuto, onClick = onAutoRamCache, label = { Text("Auto") })
                    }
                }
            }
            item(key = "appearance") {
                SettingsGroup("Appearance") {
                    SwitchSetting("Dark mode", null, state.darkTheme, onDarkTheme)
                    GroupDivider()
                    SwitchSetting(
                        title = "Wallpaper colors",
                        supporting = "Android 12 and newer. Off uses the palette below.",
                        checked = state.dynamicColors,
                        onCheckedChange = onDynamicColors,
                    )
                    GroupDivider()
                    ChoiceSetting(
                        title = "Palette",
                        supporting = when {
                            state.dynamicColors -> "Turn off wallpaper colors to choose one."
                            state.appTheme == AppTheme.WADA ->
                                "Grenadine Pink, Naples Yellow and Deep Slate Green, combination 166 in Sanzo Wada's A Dictionary of Color Combinations."
                            else -> null
                        },
                        options = listOf(
                            AppTheme.AERO to "Aero",
                            AppTheme.ANTIQUITY to "Antiquity",
                            AppTheme.RETRO to "Retro",
                            AppTheme.WADA to "Wada",
                        ),
                        selected = state.appTheme.takeIf { !state.dynamicColors },
                        enabled = !state.dynamicColors,
                        onSelect = onAppTheme,
                    )
                    GroupDivider()
                    SwitchSetting("Reduce motion", "Fewer animations.", state.reducedEffects, onReducedEffects)
                }
            }
            item(key = "data") {
                SettingsGroup("Data") {
                    // One button clears everything that is safe to clear.
                    val reclaimable = state.cacheBytes +
                        (state.nativeCacheSummary?.categories?.sumOf { it.reclaimableBytes } ?: 0L)
                    SettingRow(
                        title = "Cache",
                        supporting = "${formatBytes(reclaimable)} of previews and engine caches. Saved downloads are kept.",
                    ) {
                        OutlinedButton(onClick = onCleanup, enabled = reclaimable > 0L, contentPadding = CompactButtonPadding) { Text("Clear") }
                    }
                    GroupDivider()
                    SettingRow(title = "Stop all transfers", supporting = "Disconnects peers. Saved files are kept.") {
                        OutlinedButton(
                            onClick = onStopAll,
                            enabled = state.library.any { it.engineId != null && !it.isDeleting },
                            contentPadding = CompactButtonPadding,
                        ) { Text("Stop") }
                    }
                    GroupDivider()
                    SettingRow(title = "Shut down", supporting = "Stops transfers and closes the app.") {
                        OutlinedButton(onClick = onStopAllAndExit, contentPadding = CompactButtonPadding) { Text("Exit") }
                    }
                    GroupDivider()
                    SettingRow(
                        title = "Delete downloaded files",
                        titleColor = colors.error,
                        supporting = "Erases every saved download from the phone. The library list stays.",
                    ) {
                        OutlinedButton(
                            onClick = onClearAll,
                            contentPadding = CompactButtonPadding,
                            colors = ButtonDefaults.outlinedButtonColors(contentColor = colors.error),
                            border = BorderStroke(1.dp, colors.error),
                        ) { Text("Delete") }
                    }
                }
            }
            item(key = "about") {
                SettingsGroup("About") {
                    val uri = LocalUriHandler.current
                    SettingRow(
                        title = "Privacy policy",
                        supporting = "Nothing is collected. Lists every connection the app makes.",
                        onClick = { runCatching { uri.openUri(PRIVACY_POLICY_URL) } },
                    )
                    GroupDivider()
                    SettingRow(
                        title = "Source code",
                        supporting = "Free and open source.",
                        onClick = { runCatching { uri.openUri(SOURCE_CODE_URL) } },
                    )
                    GroupDivider()
                    SettingRow(title = "Version", supporting = "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})")
                }
            }
        }
    }
}

private val CompactButtonPadding = PaddingValues(horizontal = 16.dp, vertical = 6.dp)

/** A titled card of related settings. */
@Composable
private fun SettingsGroup(title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            text = title,
            style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold),
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 16.dp),
        )
        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
            shape = MaterialTheme.shapes.large,
        ) {
            content()
        }
    }
}

@Composable
private fun GroupDivider() {
    HorizontalDivider(
        modifier = Modifier.padding(horizontal = 16.dp),
        color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.4f),
    )
}

@Composable
private fun SettingText(title: String, supporting: String?, modifier: Modifier = Modifier, titleColor: Color = Color.Unspecified) {
    val colors = MaterialTheme.colorScheme
    Column(modifier, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(title, style = MaterialTheme.typography.bodyLarge, color = if (titleColor == Color.Unspecified) colors.onSurface else titleColor)
        if (!supporting.isNullOrBlank()) {
            Text(supporting, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
        }
    }
}

/** A setting with an optional control at the end; the whole row is tappable when [onClick] is set. */
@Composable
private fun SettingRow(
    title: String,
    supporting: String? = null,
    titleColor: Color = Color.Unspecified,
    onClick: (() -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
) {
    Row(
        modifier = Modifier.fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .heightIn(min = 56.dp)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        SettingText(title, supporting, Modifier.weight(1f), titleColor)
        if (trailing != null) {
            Spacer(Modifier.width(12.dp))
            trailing()
        }
    }
}

/** Tapping anywhere on the row flips the switch. */
@Composable
private fun SwitchSetting(title: String, supporting: String?, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth()
            .toggleable(value = checked, role = Role.Switch, onValueChange = onCheckedChange)
            .heightIn(min = 56.dp)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        SettingText(title, supporting, Modifier.weight(1f))
        Spacer(Modifier.width(12.dp))
        Switch(checked = checked, onCheckedChange = null)
    }
}

/** One choice from a short list, shown as chips under the title. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun <T> ChoiceSetting(
    title: String,
    supporting: String?,
    options: List<Pair<T, String>>,
    selected: T?,
    enabled: Boolean = true,
    onSelect: (T) -> Unit,
) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 8.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        SettingText(title, supporting)
        FlowRow(
            modifier = Modifier.selectableGroup(),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            options.forEach { (value, label) ->
                FilterChip(
                    selected = selected == value,
                    enabled = enabled,
                    onClick = { onSelect(value) },
                    label = { Text(label) },
                )
            }
        }
    }
}

/** The current value of a slider. */
@Composable
private fun ValueBadge(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Bold),
        color = MaterialTheme.colorScheme.onSecondaryContainer,
        modifier = Modifier
            .clip(MaterialTheme.shapes.small)
            .background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(horizontal = 10.dp, vertical = 4.dp),
    )
}

private const val PRIVACY_POLICY_URL = "https://github.com/karmugilen/torrent-player/blob/v2.4.0/PRIVACY.md"
private const val SOURCE_CODE_URL = "https://github.com/karmugilen/torrent-player"

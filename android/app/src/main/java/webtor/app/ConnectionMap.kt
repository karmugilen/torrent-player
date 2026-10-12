package webtor.app

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import webtor.core.EngineClient
import webtor.core.PeerConnection
import webtor.core.TorrentConnections

internal class SwarmHud(colors: ColorScheme) {
    val background = colors.surface
    val panel = colors.surfaceContainerLow
    val accent = colors.primary
    val secondary = colors.secondary
    val tertiary = colors.tertiary
    val text = colors.onSurface
    val muted = colors.onSurfaceVariant
    fun protocol(transport: String): Color = when (transport) {
        "TCP" -> accent
        "uTP" -> secondary
        "WebRTC" -> tertiary
        else -> muted
    }
}

internal fun PeerConnection.connectionKey() = "$transport/$address"

@Composable
fun ConnectionMapSection(entry: DownloadEntry, client: EngineClient) {
    val owner = LocalLifecycleOwner.current
    var snapshot by remember(entry.key, entry.engineId) { mutableStateOf<TorrentConnections?>(null) }
    var activity by remember(entry.key, entry.engineId) { mutableStateOf<Map<String, Boolean>>(emptyMap()) }
    var unavailable by remember(entry.key, entry.engineId) { mutableStateOf(false) }
    var worldView by rememberSaveable(entry.key) { mutableStateOf(true) }
    LaunchedEffect(entry.key, entry.engineId, entry.paused, owner) {
        snapshot = null
        activity = emptyMap()
        unavailable = false
        val id = entry.engineId ?: return@LaunchedEffect
        if (entry.paused) return@LaunchedEffect
        owner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            var previous = emptyMap<String, PeerConnection>()
            try {
                while (true) {
                    val result = withContext(Dispatchers.IO) {
                        runCatching { client.connections(id) }.getOrNull()
                    }
                    activity = buildMap {
                        result?.peers?.forEach { peer ->
                            val key = peer.connectionKey()
                            val before = previous[key] ?: return@forEach
                            if (peer.downloaded > before.downloaded) put(key, true)
                            else if (peer.uploaded > before.uploaded) put(key, false)
                        }
                    }
                    previous = result?.peers.orEmpty().associateBy { it.connectionKey() }
                    snapshot = result
                    unavailable = result == null
                    delay(1_000)
                }
            } finally {
                snapshot = null
                activity = emptyMap()
            }
        }
    }
    val peers = if (entry.paused || entry.engineId == null) emptyList() else snapshot?.peers.orEmpty()
    val status = when {
        entry.paused || snapshot?.paused == true -> "Paused · peers disconnected"
        entry.engineId == null -> "No active torrent session"
        unavailable -> "Connection map unavailable"
        snapshot == null -> "Finding connections…"
        peers.isEmpty() -> "Waiting for peers"
        else -> "${peers.size} peers connected"
    }
    val connected = peers.isNotEmpty()
    val hud = SwarmHud(MaterialTheme.colorScheme)
    Surface(
        modifier = Modifier.fillMaxWidth(), shape = RoundedCornerShape(24.dp),
        color = hud.background, border = BorderStroke(1.dp, hud.accent.copy(alpha = 0.18f)),
    ) {
        Column(
            Modifier.background(Brush.verticalGradient(listOf(hud.panel, hud.background))).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    Text("Swarm map", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                    Text(status, style = MaterialTheme.typography.bodySmall, color = hud.muted)
                }
                Surface(color = hud.accent.copy(alpha = if (connected) 0.12f else 0.04f), shape = RoundedCornerShape(6.dp)) {
                    Text(
                        if (connected) "LIVE" else "IDLE", Modifier.padding(horizontal = 8.dp, vertical = 5.dp),
                        color = if (connected) hud.accent else hud.muted,
                        fontFamily = FontFamily.Monospace, fontSize = 10.sp, letterSpacing = 1.sp,
                    )
                }
            }
            Row(
                Modifier.fillMaxWidth().selectableGroup().background(hud.background, RoundedCornerShape(10.dp)).padding(4.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                HudTab("World map", worldView, Modifier.weight(1f)) { worldView = true }
                HudTab("Connections", !worldView, Modifier.weight(1f)) { worldView = false }
            }
            if (worldView) PeerWorldMap(peers)
            else {
                ConnectionReactor(
                    peers = peers, activity = activity, countKnown = snapshot != null || entry.paused || entry.engineId == null,
                    download = entry.liveDownloadSpeed()?.let(::formatSpeed) ?: "—",
                    upload = entry.liveUploadSpeed()?.let(::formatSpeed) ?: "—",
                )
                if (peers.size > REACTOR_MAX_PEERS) {
                    Text("${REACTOR_MAX_PEERS} of ${peers.size} links shown", style = MaterialTheme.typography.labelSmall, color = hud.muted)
                }
            }
            if (connected) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                    peers.groupingBy { it.transport }.eachCount().toSortedMap().forEach { (protocol, count) ->
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Box(Modifier.size(5.dp).background(hud.protocol(protocol), RoundedCornerShape(2.dp)))
                            Text(protocol, color = hud.muted, style = MaterialTheme.typography.labelSmall)
                            Text("$count", color = hud.text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.labelSmall)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun HudTab(label: String, selected: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val hud = SwarmHud(MaterialTheme.colorScheme)
    Box(
        modifier.background(if (selected) hud.accent.copy(alpha = 0.13f) else Color.Transparent, RoundedCornerShape(8.dp))
            .selectable(selected = selected, role = Role.Tab, onClick = onClick).heightIn(min = 48.dp).padding(horizontal = 8.dp, vertical = 10.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (selected) hud.accent else hud.muted, style = MaterialTheme.typography.labelMedium)
    }
}

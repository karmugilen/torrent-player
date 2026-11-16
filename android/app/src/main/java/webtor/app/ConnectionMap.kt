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
import webtor.core.HolepunchStats
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
        else -> swarmPeerSummary(peers)
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
            // Per torrent only: this open download's counters from its own
            // connections snapshot. Never the client-wide `/stats` counters.
            if (!entry.paused && entry.engineId != null) {
                snapshot?.holepunch?.let { HolepunchBlock(holepunchRows(it, peers), hud) }
            }
        }
    }
}

/**
 * "N peers · BT1 X · BT2 Y · Z hole-punched". An engine that predates these
 * fields sends no `bittorrentVersion` or `source`; its peers are not counted
 * as BT1 or as direct, the parts it cannot know are left out instead.
 */
internal fun swarmPeerSummary(peers: List<PeerConnection>): String = buildString {
    append(if (peers.size == 1) "1 peer" else "${peers.size} peers")
    // Web seeds are HTTP sources, not swarm peers: the engine still tags them
    // "v1", so they must not count toward BT1 (or BT2).
    val swarm = peers.filter { !it.transport.equals("WebSeed", ignoreCase = true) }
    if (swarm.any { it.bittorrentVersion != null }) {
        val bt1 = swarm.count { it.bittorrentVersion == "v1" }
        val bt2 = swarm.count { it.isBt2 }
        append(" · BT1 $bt1 · BT2 $bt2")
    }
    if (peers.any { it.source != null }) {
        append(" · ${holePunchedNow(peers)} hole-punched")
    }
}

/** Peers on this torrent reached by BEP 55 (source "C"); the swarm line and the block share it. */
internal fun holePunchedNow(peers: List<PeerConnection>): Int = peers.count { it.holePunched }

/**
 * The hole punching block for one torrent: "Hole-punched now" is the same count
 * as the swarm line's "Z hole-punched", then this torrent's monotonic counters.
 */
internal fun holepunchRows(stats: HolepunchStats, peers: List<PeerConnection>): List<Pair<String, Long>> =
    listOf("Hole-punched now" to holePunchedNow(peers).toLong()) + stats.rows()

/** Label and value rows for the BEP 55 counters, in display order. */
internal fun HolepunchStats.rows(): List<Pair<String, Long>> = listOf(
    "Rendezvous sent" to rendezvousSent,
    "Connects received" to connectReceived,
    "Relayed for others" to relayedForOthers,
    "Errors received" to errorsReceived,
    "Punch dials" to punchDialsAttempted,
    "Punched peers" to punchDialsSucceeded,
)

@Composable
private fun HolepunchBlock(rows: List<Pair<String, Long>>, hud: SwarmHud) {
    Column(
        Modifier.fillMaxWidth().background(hud.background, RoundedCornerShape(10.dp)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("Hole punching (BEP 55)", Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = hud.text)
            Text("this torrent", color = hud.muted, style = MaterialTheme.typography.labelSmall)
        }
        rows.chunked(2).forEach { pair ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                pair.forEach { (label, value) ->
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        Text(label, Modifier.weight(1f), color = hud.muted, style = MaterialTheme.typography.labelSmall)
                        Text("$value", color = hud.text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.labelSmall)
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

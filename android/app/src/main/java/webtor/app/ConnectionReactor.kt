package webtor.app

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.*
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import webtor.core.PeerConnection

private const val DEFAULT_PEERS_PER_GROUP = 4

@Composable
internal fun ConnectionReactor(
    peers: List<PeerConnection>,
    activity: Map<String, Boolean>,
    countKnown: Boolean,
    download: String,
    upload: String,
    modifier: Modifier = Modifier,
) {
    val hud = SwarmHud(MaterialTheme.colorScheme)
    val reducedEffects = LocalReducedEffects.current
    val groups = remember(peers) {
        peers.groupBy { it.transport }
            .toSortedMap()
            .mapValues { (_, list) ->
                list.sortedWith(compareBy({ it.address }, { it.client }))
            }
    }
    val activeCount = remember(peers, activity) {
        peers.count { it.connectionKey() in activity }
    }

    val semanticSummary = remember(peers, countKnown, download, upload, groups) {
        if (!countKnown) "Loading peer connections."
        else if (peers.isEmpty()) "This device connected to 0 peers. Download $download, upload $upload."
        else "This device connected to ${peers.size} peers across ${groups.keys.joinToString()} protocols. $activeCount transferring. Download $download, upload $upload."
    }

    Column(
        modifier = modifier
            .fillMaxWidth()
            .semantics { contentDescription = semanticSummary },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(0.dp),
    ) {
        // UI-04: Device node at top
        DeviceRootNode(download = download, upload = upload, hud = hud, totalPeers = peers.size)

        if (peers.isEmpty()) {
            VerticalBranchConnector(height = 20.dp, color = hud.accent.copy(alpha = 0.25f))
            Surface(
                shape = RoundedCornerShape(12.dp),
                color = hud.panel,
                border = BorderStroke(1.dp, hud.muted.copy(alpha = 0.2f)),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
            ) {
                Box(
                    modifier = Modifier.padding(16.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (!countKnown) "Searching for swarm connections…" else "No peers connected yet",
                        style = MaterialTheme.typography.bodySmall,
                        color = hud.muted,
                    )
                }
            }
        } else {
            VerticalBranchConnector(height = 16.dp, color = hud.accent.copy(alpha = 0.25f))

            // Protocol groups branching out
            Column(
                modifier = Modifier.fillMaxWidth(),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                groups.forEach { (transport, groupPeers) ->
                    ProtocolGroupSection(
                        transport = transport,
                        peers = groupPeers,
                        activity = activity,
                        hud = hud,
                        reducedEffects = reducedEffects,
                    )
                }
            }
        }
    }
}

@Composable
private fun DeviceRootNode(
    download: String,
    upload: String,
    hud: SwarmHud,
    totalPeers: Int,
) {
    Surface(
        shape = RoundedCornerShape(16.dp),
        color = hud.panel,
        border = BorderStroke(1.5.dp, hud.accent.copy(alpha = 0.35f)),
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Box(
                        modifier = Modifier
                            .size(8.dp)
                            .clip(CircleShape)
                            .background(if (totalPeers > 0) hud.accent else hud.muted)
                    )
                    Text(
                        "THIS DEVICE",
                        style = MaterialTheme.typography.labelSmall.copy(
                            fontWeight = FontWeight.Bold,
                            letterSpacing = 1.2.sp,
                            fontFamily = FontFamily.Monospace,
                        ),
                        color = hud.text,
                    )
                }
                Text(
                    "$totalPeers connected peers",
                    style = MaterialTheme.typography.bodySmall,
                    color = hud.muted,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                RateColumn(label = "DOWN", rate = download, color = hud.accent)
                RateColumn(label = "UP", rate = upload, color = hud.secondary)
            }
        }
    }
}

@Composable
private fun RateColumn(label: String, rate: String, color: Color) {
    Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(1.dp)) {
        Text(
            label,
            style = MaterialTheme.typography.labelSmall.copy(
                fontSize = 8.sp,
                fontFamily = FontFamily.Monospace,
                letterSpacing = 1.sp,
            ),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            rate,
            style = MaterialTheme.typography.labelMedium.copy(
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
            ),
            color = color,
        )
    }
}

@Composable
private fun ProtocolGroupSection(
    transport: String,
    peers: List<PeerConnection>,
    activity: Map<String, Boolean>,
    hud: SwarmHud,
    reducedEffects: Boolean,
) {
    var expanded by remember { mutableStateOf(false) }
    val protocolColor = hud.protocol(transport)
    val displayedPeers = if (expanded || peers.size <= DEFAULT_PEERS_PER_GROUP) peers else peers.take(DEFAULT_PEERS_PER_GROUP)
    val hasMore = peers.size > DEFAULT_PEERS_PER_GROUP

    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        // Protocol Branch Header
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Surface(
                    shape = RoundedCornerShape(6.dp),
                    color = protocolColor.copy(alpha = 0.15f),
                    border = BorderStroke(1.dp, protocolColor.copy(alpha = 0.4f)),
                ) {
                    Text(
                        transport,
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
                        style = MaterialTheme.typography.labelSmall.copy(
                            fontWeight = FontWeight.Bold,
                            fontFamily = FontFamily.Monospace,
                        ),
                        color = protocolColor,
                    )
                }
                Text(
                    "${peers.size} ${if (peers.size == 1) "peer" else "peers"}",
                    style = MaterialTheme.typography.labelSmall,
                    color = hud.muted,
                )
            }

            if (hasMore) {
                TextButton(
                    onClick = { expanded = !expanded },
                    contentPadding = PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                ) {
                    Text(
                        if (expanded) "Show less" else "+${peers.size - DEFAULT_PEERS_PER_GROUP} more",
                        style = MaterialTheme.typography.labelSmall,
                        color = hud.accent,
                    )
                }
            }
        }

        // Rectangular peer tiles
        Column(
            modifier = Modifier.fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            displayedPeers.forEach { peer ->
                val peerTransfer = activity[peer.connectionKey()]
                RectangularPeerTile(
                    peer = peer,
                    transferring = peerTransfer,
                    protocolColor = protocolColor,
                    hud = hud,
                    reducedEffects = reducedEffects,
                )
            }
        }
    }
}

@Composable
private fun RectangularPeerTile(
    peer: PeerConnection,
    transferring: Boolean?,
    protocolColor: Color,
    hud: SwarmHud,
    reducedEffects: Boolean,
) {
    val isActive = transferring != null
    val borderAlpha = if (isActive) 0.5f else 0.15f
    val borderColor = if (isActive) protocolColor else hud.muted

    Surface(
        shape = RoundedCornerShape(10.dp),
        color = hud.panel,
        border = BorderStroke(1.dp, borderColor.copy(alpha = borderAlpha)),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column(
                modifier = Modifier.weight(1f).padding(end = 8.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    // Country badge if available
                    peer.country?.takeIf { it.isNotBlank() }?.let { country ->
                        Surface(
                            shape = RoundedCornerShape(4.dp),
                            color = hud.background,
                            border = BorderStroke(0.5.dp, hud.muted.copy(alpha = 0.3f)),
                        ) {
                            Text(
                                country.uppercase(),
                                modifier = Modifier.padding(horizontal = 4.dp, vertical = 1.dp),
                                style = MaterialTheme.typography.labelSmall.copy(
                                    fontSize = 9.sp,
                                    fontFamily = FontFamily.Monospace,
                                ),
                                color = hud.text,
                            )
                        }
                    }

                    // Client identifier
                    Text(
                        peer.client.ifBlank { peer.address },
                        style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight.Medium),
                        color = hud.text,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }

                // Address & transfer totals
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        peer.address,
                        style = MaterialTheme.typography.labelSmall.copy(fontFamily = FontFamily.Monospace),
                        color = hud.muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        "↓ ${formatBytes(peer.downloaded)} · ↑ ${formatBytes(peer.uploaded)}",
                        style = MaterialTheme.typography.labelSmall.copy(fontSize = 10.sp),
                        color = hud.muted,
                    )
                }
            }

            // Real activity status (no fake animation on idle)
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                val statusDotColor = when (transferring) {
                    true -> hud.accent
                    false -> hud.secondary
                    null -> Color.Transparent
                }
                if (transferring != null) {
                    Box(
                        modifier = Modifier
                            .size(6.dp)
                            .clip(CircleShape)
                            .background(statusDotColor)
                    )
                }
                Text(
                    text = when (transferring) {
                        true -> "Receiving"
                        false -> "Sending"
                        null -> "Idle"
                    },
                    style = MaterialTheme.typography.labelSmall.copy(
                        fontFamily = FontFamily.Monospace,
                        fontWeight = if (isActive) FontWeight.SemiBold else FontWeight.Normal,
                    ),
                    color = when (transferring) {
                        true -> hud.accent
                        false -> hud.secondary
                        null -> hud.muted
                    },
                )
            }
        }
    }
}

@Composable
private fun VerticalBranchConnector(height: androidx.compose.ui.unit.Dp, color: Color) {
    Canvas(
        modifier = Modifier
            .width(2.dp)
            .height(height)
    ) {
        drawLine(
            color = color,
            start = Offset(size.width / 2, 0f),
            end = Offset(size.width / 2, size.height),
            strokeWidth = 2.dp.toPx(),
        )
    }
}

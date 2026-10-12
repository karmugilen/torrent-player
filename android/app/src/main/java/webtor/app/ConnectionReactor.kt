package webtor.app

import androidx.compose.animation.core.*
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import webtor.core.PeerConnection
import kotlin.math.cos
import kotlin.math.sin

internal const val REACTOR_MAX_PEERS = 18

@Composable
internal fun ConnectionReactor(
    peers: List<PeerConnection>, activity: Map<String, Boolean>, countKnown: Boolean,
    download: String, upload: String,
) {
    val hud = SwarmHud(MaterialTheme.colorScheme)
    val shown = remember(peers) { peers.sortedWith(compareBy({ it.transport }, { it.address })).take(REACTOR_MAX_PEERS) }
    // Animate only peers whose payload counters changed; idle views stay static.
    // Canvas reads the frame state directly, without recomposing the screen.
    val phase: State<Float> = if (activity.isNotEmpty() && shown.any { it.connectionKey() in activity }) {
        rememberInfiniteTransition(label = "Peer data flow").animateFloat(
            initialValue = 0f, targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(1_800, easing = LinearEasing), RepeatMode.Restart),
            label = "Payload pulse",
        )
    } else remember { mutableFloatStateOf(0f) }
    Box(Modifier.fillMaxWidth().height(288.dp)) {
        Canvas(Modifier.fillMaxSize().semantics {
            contentDescription = if (countKnown) "This device connected to ${peers.size} peers. Download $download, upload $upload."
            else "Loading peer connections."
        }) {
            val center = Offset(size.width / 2, size.height / 2 + 8.dp.toPx())
            val outer = minOf(size.width / 2 - 20.dp.toPx(), size.height / 2 - 24.dp.toPx()).coerceAtLeast(0f)
            val core = minOf(42.dp.toPx(), outer * 0.42f)
            drawCircle(
                Brush.radialGradient(listOf(hud.accent.copy(alpha = 0.09f), hud.background.copy(alpha = 0f)), center, outer.coerceAtLeast(1f)),
                outer, center,
            )
            val grid = 24.dp.toPx()
            var x = 0f
            while (x < size.width) {
                drawLine(hud.accent.copy(alpha = 0.035f), Offset(x, 38.dp.toPx()), Offset(x, size.height))
                x += grid
            }
            var y = 38.dp.toPx()
            while (y < size.height) {
                drawLine(hud.accent.copy(alpha = 0.035f), Offset(0f, y), Offset(size.width, y))
                y += grid
            }
            drawCircle(hud.accent.copy(alpha = 0.08f), outer - 10.dp.toPx(), center, style = Stroke(1.dp.toPx()))
            for (tick in 0 until 72) {
                val angle = tick * Math.PI * 2 / 72
                val direction = Offset(cos(angle).toFloat(), sin(angle).toFloat())
                val major = tick % 6 == 0
                drawLine(
                    hud.accent.copy(alpha = if (major) 0.32f else 0.12f), center + direction * outer,
                    center + direction * (outer - (if (major) 6.dp.toPx() else 2.dp.toPx())), strokeWidth = 1.dp.toPx(),
                )
            }
            shown.forEachIndexed { index, peer ->
                val angle = -Math.PI / 2 + (index + 0.5) * Math.PI * 2 / shown.size
                val direction = Offset(cos(angle).toFloat(), sin(angle).toFloat())
                val tangent = Offset(-direction.y, direction.x)
                val start = center + direction * (core + 9.dp.toPx())
                val end = center + direction * (outer - 18.dp.toPx())
                val control = center + direction * (outer * 0.64f) + tangent * 13.dp.toPx()
                val route = Path().apply {
                    moveTo(start.x, start.y)
                    quadraticTo(control.x, control.y, end.x, end.y)
                }
                val color = hud.protocol(peer.transport)
                val receiving = activity[peer.connectionKey()]
                drawPath(route, color.copy(alpha = if (receiving != null) 0.4f else 0.18f), style = Stroke(1.dp.toPx()))
                drawCircle(color.copy(alpha = 0.09f), 11.dp.toPx(), end)
                drawCircle(color.copy(alpha = 0.48f), 6.dp.toPx(), end, style = Stroke(1.dp.toPx()))
                drawCircle(color, 2.5.dp.toPx(), end)
                if (receiving != null) {
                    val progress = (phase.value + index * 0.17f) % 1f
                    val t = if (receiving) 1f - progress else progress
                    val position = start * ((1f - t) * (1f - t)) + control * (2 * (1f - t) * t) + end * (t * t)
                    drawCircle(color.copy(alpha = 0.16f), 7.dp.toPx(), position)
                    drawCircle(color, 2.dp.toPx(), position)
                }
            }
            drawCircle(hud.background, core + 7.dp.toPx(), center)
            drawCircle(hud.accent.copy(alpha = 0.07f), core, center)
            drawCircle(hud.accent.copy(alpha = 0.27f), core, center, style = Stroke(1.dp.toPx()))
            val ring = core + 5.dp.toPx()
            for (segment in 0 until 3) {
                drawArc(
                    hud.accent, startAngle = segment * 120f - 90, sweepAngle = 78f, useCenter = false,
                    topLeft = center - Offset(ring, ring), size = Size(ring * 2, ring * 2), style = Stroke(2.dp.toPx()),
                )
            }
            val inner = core - 8.dp.toPx()
            drawArc(
                hud.accent.copy(alpha = 0.15f), startAngle = 20f, sweepAngle = 240f, useCenter = false,
                topLeft = center - Offset(inner, inner), size = Size(inner * 2, inner * 2), style = Stroke(1.dp.toPx()),
            )
            val length = 10.dp.toPx()
            for (corner in listOf(Offset(0f, 42.dp.toPx()), Offset(size.width, 42.dp.toPx()), Offset(0f, size.height), Offset(size.width, size.height))) {
                val dx = if (corner.x == 0f) length else -length
                val dy = if (corner.y == size.height) -length else length
                drawLine(hud.accent.copy(alpha = 0.36f), corner, corner + Offset(dx, 0f), 1.dp.toPx())
                drawLine(hud.accent.copy(alpha = 0.36f), corner, corner + Offset(0f, dy), 1.dp.toPx())
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            ReactorReadout("RECEIVING", download, Alignment.Start)
            ReactorReadout("SENDING", upload, Alignment.End)
        }
        Column(Modifier.align(Alignment.Center).offset(y = 8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(if (countKnown) peers.size.toString() else "—", color = hud.text, fontSize = 30.sp, fontWeight = FontWeight.Light, fontFamily = FontFamily.Monospace)
            Text("PEERS", color = hud.accent, fontSize = 9.sp, letterSpacing = 2.sp, fontFamily = FontFamily.Monospace)
        }
        Text(
            "THIS DEVICE", Modifier.align(Alignment.BottomCenter).padding(bottom = 3.dp),
            color = hud.muted, fontSize = 9.sp, letterSpacing = 2.sp, fontFamily = FontFamily.Monospace,
        )
    }
}

@Composable
private fun ReactorReadout(label: String, value: String, alignment: Alignment.Horizontal) {
    val hud = SwarmHud(MaterialTheme.colorScheme)
    Column(horizontalAlignment = alignment, verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(label, color = hud.muted, fontFamily = FontFamily.Monospace, fontSize = 9.sp, letterSpacing = 1.sp)
        Text(value, color = hud.accent, style = MaterialTheme.typography.labelMedium, fontFamily = FontFamily.Monospace)
    }
}

package webtor.app

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import webtor.core.PeerConnection
import kotlin.math.abs

private data class MapCountry(
    val code: String,
    val name: String,
    val longitude: Float,
    val latitude: Float,
    val rings: List<List<Offset>>,
)

// Equirectangular projection; dots indicate country label points, not GPS fixes.
private fun project(longitude: Float, latitude: Float, size: Size): Offset =
    Offset((longitude + 180f) / 360f * size.width, (90f - latitude) / 180f * size.height)

@Composable
fun PeerWorldMap(peers: List<PeerConnection>) {
    val context = LocalContext.current
    var countries by remember { mutableStateOf<List<MapCountry>?>(null) }
    var loadFailed by remember { mutableStateOf(false) }
    var selectedCode by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        countries = withContext(Dispatchers.IO) {
            runCatching {
                val json = context.assets.open("world-map.json").bufferedReader().use { JSONArray(it.readText()) }
                List(json.length()) { index ->
                    val country = json.getJSONObject(index)
                    val rings = country.getJSONArray("rings")
                    MapCountry(
                        country.getString("code"), country.getString("name"),
                        country.getDouble("longitude").toFloat(), country.getDouble("latitude").toFloat(),
                        List(rings.length()) { ringIndex ->
                            val ring = rings.getJSONArray(ringIndex)
                            List(ring.length()) { pointIndex ->
                                val point = ring.getJSONArray(pointIndex)
                                Offset(point.getDouble(0).toFloat(), point.getDouble(1).toFloat())
                            }
                        },
                    )
                }
            }.getOrNull()
        }
        loadFailed = countries == null
    }
    val colors = MaterialTheme.colorScheme
    val counts = peers.mapNotNull { it.country }.groupingBy { it }.eachCount()
    val mapped = countries.orEmpty().filter { it.code in counts }
    val locatedCount = mapped.sumOf { counts[it.code] ?: 0 }
    val unknown = peers.size - locatedCount
    val summary = mapped.sortedByDescending { counts[it.code] }.joinToString { "${it.name}: ${counts[it.code]}" }
    Canvas(
        Modifier.fillMaxWidth().aspectRatio(2f)
            .clip(RoundedCornerShape(12.dp))
            .border(1.dp, colors.primary.copy(alpha = 0.12f), RoundedCornerShape(12.dp)).semantics {
            contentDescription = "World map of approximate peer countries. $summary. $unknown peers with unknown location."
        }.pointerInput(mapped, counts) {
            detectTapGestures { tap ->
                val dimensions = Size(size.width.toFloat(), size.height.toFloat())
                val nearest = mapped.minByOrNull { country ->
                    (project(country.longitude, country.latitude, dimensions) - tap).getDistanceSquared()
                }
                selectedCode = nearest?.takeIf {
                    (project(it.longitude, it.latitude, dimensions) - tap).getDistance() < 24.dp.toPx()
                }?.code
            }
        },
    ) {
        drawRect(Brush.radialGradient(
            listOf(colors.primary.copy(alpha = 0.05f), colors.surface),
            center = Offset(size.width / 2, size.height / 2), radius = size.width.coerceAtLeast(1f),
        ))
        for (longitude in -120..120 step 60) {
            val x = project(longitude.toFloat(), 0f, size).x
            drawLine(colors.outlineVariant.copy(alpha = 0.35f), Offset(x, 0f), Offset(x, size.height))
        }
        for (latitude in -60..60 step 30) {
            val y = project(0f, latitude.toFloat(), size).y
            drawLine(colors.outlineVariant.copy(alpha = 0.35f), Offset(0f, y), Offset(size.width, y))
        }
        countries.orEmpty().forEach { country ->
            country.rings.forEach { ring ->
                val path = Path()
                ring.forEachIndexed { index, point ->
                    val position = project(point.x, point.y, size)
                    if (index == 0 || abs(point.x - ring[index - 1].x) > 180f) {
                        path.moveTo(position.x, position.y)
                    } else path.lineTo(position.x, position.y)
                }
                path.close()
                drawPath(path, if (country.code in counts) colors.primary.copy(alpha = 0.18f) else colors.surfaceVariant)
                drawPath(path, colors.primary.copy(alpha = if (country.code in counts) 0.4f else 0.12f), style = Stroke(0.5.dp.toPx()))
            }
        }
        mapped.forEach { country ->
            val point = project(country.longitude, country.latitude, size)
            val radius = (3f + (counts[country.code] ?: 1).coerceAtMost(12) * 0.4f).dp.toPx()
            drawCircle(colors.primary.copy(alpha = 0.2f), radius + 4.dp.toPx(), point)
            drawCircle(colors.primary.copy(alpha = 0.6f), radius + 2.dp.toPx(), point, style = Stroke(0.75.dp.toPx()))
            drawCircle(colors.primary, radius * 0.65f, point)
            if (selectedCode == country.code) drawCircle(colors.onSurface, radius + 3.dp.toPx(), point, style = Stroke(1.dp.toPx()))
        }
    }
    val selected = mapped.firstOrNull { it.code == selectedCode }
    Text(
        when {
            loadFailed -> "World map unavailable"
            countries == null -> "Loading world map…"
            selected != null -> "${selected.name} · ${counts[selected.code]} connected peers"
            mapped.isNotEmpty() -> "${mapped.size} countries · $locatedCount located peers · $unknown unknown"
            else -> "No peer locations available"
        },
        style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
    )
    Text("Country estimates · tap a marker", style = MaterialTheme.typography.labelSmall, color = colors.onSurfaceVariant)
    Text(
        "IP Geolocation by DB-IP · offline data, Oct 2026",
        modifier = Modifier.clickable { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://db-ip.com"))) },
        style = MaterialTheme.typography.labelSmall, color = colors.primary,
    )
}

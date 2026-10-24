package webtor.app

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import webtor.core.PieceTelemetry

/**
 * Material 3 soft-tile heat grid for torrent piece telemetry.
 * Dense rounded cells with clear waiting contrast; mixed bands share one clip.
 */
@Composable
fun PieceHeatGridSection(
    map: PieceTelemetry?,
    frozen: Boolean,
    files: List<SavedFile> = emptyList(),
    memoryBuffer: Boolean = false,
) {
    val colors = MaterialTheme.colorScheme
    val pieceColors = LocalPieceColors.current
    val density = LocalDensity.current
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (map == null || map.buckets.isEmpty()) {
            Text(
                if (memoryBuffer) "RAM pieces" else "Pieces",
                style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                color = colors.onBackground,
            )
            Card(
                modifier = Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(12.dp),
                colors = CardDefaults.cardColors(containerColor = colors.surfaceVariant.copy(alpha = 0.45f)),
                elevation = CardDefaults.cardElevation(defaultElevation = 0.dp),
            ) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(
                        "Map unavailable",
                        style = MaterialTheme.typography.bodyMedium,
                        color = colors.onSurface,
                    )
                    Text(
                        "Engine offline or telemetry not ready yet",
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.onSurfaceVariant,
                    )
                }
            }
            return
        }

        val buckets = remember(map) { normalizePieceBuckets(map) }
        val summary = remember(map, buckets) { summarizePieceHeat(map, buckets) }
        val playback = remember(map, buckets) {
            buckets.map { if (map.ghostProMode) playbackBands(it, map.playback) else emptyList() }
        }
        val hasPlayback = playback.any { it.isNotEmpty() }
        val layout = remember(buckets.size) { pieceHeatLayout(buckets.size) }
        // Blocks being fetched breathe between two amber strengths so activity is visible.
        val pulse = if (summary.selectedReceiving > 0 && !frozen) {
            val transition = rememberInfiniteTransition(label = "pieceReceiving")
            transition.animateFloat(
                initialValue = 0.55f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(tween(durationMillis = 700), RepeatMode.Reverse),
                label = "pieceReceivingAlpha",
            )
        } else null
        val markers = remember(files, map.pieceLength, buckets) {
            fileBoundaryMarkers(files, map.pieceLength, buckets)
        }
        var peekIndex by remember(map.id, map.generation) { mutableStateOf<Int?>(null) }
        var canvasWidthPx by remember { mutableStateOf(0f) }
        val heightDp = remember(layout, canvasWidthPx, density) {
            pieceHeatHeightDp(layout, canvasWidthPx, density.density).dp
        }
        val a11y = remember(summary, frozen, memoryBuffer, hasPlayback) {
            accessibilityPieceHeatSummary(summary, memoryBuffer) +
                (if (frozen) " Map paused." else "") +
                (if (hasPlayback) " Ghost Pro playback: coral urgent, blue readahead, purple lookahead. Tinted outline means waiting, solid means downloaded." else "")
        }
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                if (memoryBuffer) "RAM pieces" else "Pieces",
                style = MaterialTheme.typography.titleSmall.copy(fontWeight = FontWeight.SemiBold),
                color = colors.onBackground,
                modifier = Modifier.weight(1f),
            )
            Text(
                summary.titleMeta(),
                style = MaterialTheme.typography.labelSmall,
                color = colors.onSurfaceVariant,
            )
            Surface(
                shape = RoundedCornerShape(999.dp),
                color = colors.primaryContainer,
                contentColor = colors.onPrimaryContainer,
            ) {
                Text(
                    text = summary.verifiedPercent?.let { "$it%" } ?: "—",
                    style = MaterialTheme.typography.labelMedium.copy(fontWeight = FontWeight.Bold),
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
                )
            }
        }

        Card(
            modifier = Modifier
                .fillMaxWidth()
                .semantics { contentDescription = a11y },
            shape = RoundedCornerShape(12.dp),
            colors = CardDefaults.cardColors(containerColor = colors.surface),
            elevation = CardDefaults.cardElevation(defaultElevation = 1.dp),
            border = BorderStroke(1.dp, colors.outlineVariant.copy(alpha = 0.55f)),
        ) {
            Column(
                Modifier
                    .padding(8.dp)
                    .pointerInput(Unit) {
                        detectTapGestures { peekIndex = null }
                    },
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                // Done = primary teal; Getting/working = bright amber so workers pop.
                val verifiedColor = pieceColors.done ?: colors.primary
                val receivingColor = pieceColors.getting
                val receivingGlow = pieceColors.gettingGlow
                val missingStroke = colors.outline.copy(alpha = 0.35f)
                val excludedStroke = colors.outline.copy(alpha = 0.55f)
                val excludedFill = colors.surfaceVariant.copy(alpha = 0.25f)
                val boardColor = colors.surfaceVariant.copy(alpha = 0.55f)

                Box(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(12.dp))
                        .background(boardColor)
                        .padding(8.dp),
                ) {
                    Canvas(
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(heightDp)
                            .onSizeChanged { canvasWidthPx = it.width.toFloat() }
                            .pointerInput(buckets, layout) {
                                detectTapGestures { offset ->
                                    val hit = hitTestPieceCell(
                                        x = offset.x,
                                        y = offset.y,
                                        width = size.width.toFloat(),
                                        height = size.height.toFloat(),
                                        layout = layout,
                                    )
                                    peekIndex = hit
                                }
                            },
                    ) {
                        val count = buckets.size
                        if (count == 0) return@Canvas
                        val cols = layout.columns
                        val (cell, gap) = pieceHeatSquareCell(size.width, size.height, layout)
                        if (cell <= 0f) return@Canvas
                        val radius = CornerRadius(
                            x = (cell * 0.28f).coerceIn(2.5f, 6f),
                            y = (cell * 0.28f).coerceIn(2.5f, 6f),
                        )
                        val stroke = Stroke(width = 1.1f)
                        val activeStroke = Stroke(width = 1.8f)
                        val receivingAlpha = pulse?.value ?: 1f

                        // One solid color per tile: state and progress are read from color,
                        // so tiles stay small and legible at the dense 32-column layout.
                        buckets.forEachIndexed { index, bucket ->
                            val col = index % cols
                            val row = index / cols
                            val origin = Offset(col * (cell + gap), row * (cell + gap))
                            val cellSize = Size(cell, cell)

                            if (bucket.excludedOnly) {
                                drawRoundRect(color = excludedFill, topLeft = origin, size = cellSize, cornerRadius = radius)
                                drawRoundRect(
                                    color = excludedStroke, topLeft = origin, size = cellSize,
                                    cornerRadius = radius, style = stroke,
                                )
                                return@forEachIndexed
                            }

                            val progress = if (bucket.selected > 0) {
                                bucket.selectedVerified.toFloat() / bucket.selected
                            } else 0f
                            val working = bucket.selectedReceiving > 0
                            val priority = playback[index].maxByOrNull { playbackRank(it.priority) }?.priority
                            val tint = priority?.let { pieceColors.playback(it) } ?: verifiedColor

                            val fill = when {
                                working -> receivingColor.copy(alpha = receivingAlpha)
                                progress >= 1f -> tint
                                // Partial blocks deepen toward the done color as pieces land.
                                progress > 0f -> tint.copy(alpha = 0.22f + 0.6f * progress)
                                priority != null -> tint.copy(alpha = 0.16f)
                                else -> null
                            }
                            if (fill != null) {
                                drawRoundRect(color = fill, topLeft = origin, size = cellSize, cornerRadius = radius)
                            }
                            when {
                                working -> drawRoundRect(
                                    color = if (priority != null) tint else receivingGlow,
                                    topLeft = origin, size = cellSize, cornerRadius = radius, style = activeStroke,
                                )
                                progress < 1f -> drawRoundRect(
                                    color = if (priority != null) tint.copy(alpha = 0.85f) else missingStroke,
                                    topLeft = origin, size = cellSize, cornerRadius = radius,
                                    style = if (priority != null) activeStroke else stroke,
                                )
                            }
                        }

                        val tickColor = colors.onSurfaceVariant.copy(alpha = 0.75f)
                        for (marker in markers) {
                            val col = marker.bucketIndex % cols
                            val row = marker.bucketIndex / cols
                            val left = col * (cell + gap)
                            val top = row * (cell + gap)
                            val tickY = top + cell + minOf(gap * 0.4f, 2.5f)
                            drawLine(
                                color = tickColor,
                                start = Offset(left + cell * 0.2f, tickY),
                                end = Offset(left + cell * 0.8f, tickY),
                                strokeWidth = 1.6f,
                                cap = StrokeCap.Round,
                            )
                        }
                    }
                }

                PieceHeatLegend(summary = summary, memoryBuffer = memoryBuffer)
                if (map.ghostProMode) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        for ((priority, label) in listOf("U" to "Urgent", "R" to "Readahead", "L" to "Lookahead")) {
                            LegendPill(
                                color = pieceColors.playback(priority), hollow = false, label = label,
                                count = playback.flatten().filter { it.priority == priority }.sumOf { it.pieces },
                                modifier = Modifier.weight(1f),
                            )
                        }
                    }
                    Text(
                        if (hasPlayback) "Ghost Pro · Playback window tiles fill with their color as pieces arrive; outlined tiles are still waiting. Tap for counts."
                        else "Ghost Pro · Play a video to see its urgent, readahead and lookahead windows.",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.onSurfaceVariant,
                    )
                }
                if (memoryBuffer) {
                    Text(
                        "Filled = held in RAM. Evicted pieces become empty. Blocks can group several pieces.",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.onSurfaceVariant,
                    )
                }

                val peek = peekIndex?.let { idx -> buckets.getOrNull(idx)?.let { peekForBucket(it) } }
                if (peek != null) {
                    Surface(
                        shape = RoundedCornerShape(10.dp),
                        color = colors.secondaryContainer.copy(alpha = 0.65f),
                        contentColor = colors.onSecondaryContainer,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text(
                            peek.line(memoryBuffer) + playback.getOrNull(peek.bucketIndex).orEmpty()
                                .groupBy { it.priority }.entries.joinToString(separator = "", transform = { (priority, bands) ->
                                    " · ${playbackLabel(priority)} ${bands.sumOf { it.pieces }}"
                                }),
                            style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                        )
                    }
                }
            }
        }
    }
}

private fun playbackLabel(priority: String): String = when (priority) {
    "U" -> "urgent"
    "R" -> "readahead"
    else -> "lookahead"
}

private fun PieceColors.playback(priority: String): Color = when (priority) {
    "U" -> urgent
    "R" -> readahead
    else -> lookahead
}

@Composable
private fun PieceHeatLegend(summary: PieceHeatSummary, memoryBuffer: Boolean) {
    val colors = MaterialTheme.colorScheme
    val pieceColors = LocalPieceColors.current
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LegendPill(
            color = pieceColors.done ?: colors.primary,
            hollow = false,
            label = if (memoryBuffer) "Buffered" else "Done",
            count = summary.selectedVerified,
            modifier = Modifier.weight(1f),
        )
        LegendPill(
            color = pieceColors.getting,
            hollow = false,
            label = "Getting",
            count = summary.selectedReceiving,
            modifier = Modifier.weight(1f),
        )
        LegendPill(
            color = colors.outlineVariant,
            hollow = true,
            label = if (memoryBuffer) "Empty" else "Waiting",
            count = summary.missing,
            modifier = Modifier.weight(1f),
        )
        if (summary.excluded > 0) {
            LegendPill(
                color = colors.outline,
                hollow = true,
                label = "Off",
                count = summary.excluded,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

@Composable
private fun LegendPill(
    color: Color,
    hollow: Boolean,
    label: String,
    count: Int,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(999.dp),
        color = colors.surfaceVariant.copy(alpha = 0.55f),
        contentColor = colors.onSurfaceVariant,
    ) {
        Row(
            Modifier.padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(
                Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(if (hollow) Color.Transparent else color)
                    .then(
                        if (hollow) Modifier.border(BorderStroke(1.25.dp, color), CircleShape)
                        else Modifier,
                    ),
            )
            Text(
                "$label $count",
                style = MaterialTheme.typography.labelSmall.copy(fontWeight = FontWeight.Medium),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

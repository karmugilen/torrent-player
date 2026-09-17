package webtor.app

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
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
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
) {
    val colors = MaterialTheme.colorScheme
    val density = LocalDensity.current
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (map == null || map.buckets.isEmpty()) {
            Text(
                "Pieces",
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
        val layout = remember(buckets.size) { pieceHeatLayout(buckets.size) }
        val markers = remember(files, map.pieceLength, buckets) {
            fileBoundaryMarkers(files, map.pieceLength, buckets)
        }
        var peekIndex by remember(map.id, map.generation) { mutableStateOf<Int?>(null) }
        var canvasWidthPx by remember { mutableStateOf(0f) }
        val heightDp = remember(layout, canvasWidthPx, density) {
            pieceHeatHeightDp(layout, canvasWidthPx, density.density).dp
        }
        val a11y = remember(summary, frozen) {
            accessibilityPieceHeatSummary(summary) + if (frozen) " Map frozen." else ""
        }
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "Pieces",
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
                val verifiedColor = colors.primary
                val receivingColor = Color(0xFFFFB300)
                val receivingGlow = Color(0xFFFF6F00)
                val missingColor = colors.outlineVariant.copy(alpha = 0.72f)
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
                        val activeStroke = Stroke(width = 2.2f)

                        buckets.forEachIndexed { index, bucket ->
                            val col = index % cols
                            val row = index / cols
                            val left = col * (cell + gap)
                            val top = row * (cell + gap)
                            val cellSize = Size(cell, cell)
                            val origin = Offset(left, top)
                            val working = bucket.selectedReceiving > 0

                            if (bucket.excludedOnly) {
                                drawRoundRect(
                                    color = excludedFill,
                                    topLeft = origin,
                                    size = cellSize,
                                    cornerRadius = radius,
                                )
                                drawRoundRect(
                                    color = excludedStroke,
                                    topLeft = origin,
                                    size = cellSize,
                                    cornerRadius = radius,
                                    style = stroke,
                                )
                                return@forEachIndexed
                            }

                            fun band(fraction: Float): Float =
                                (cell * fraction.coerceIn(0f, 1f)).coerceAtLeast(0f)

                            var vH = band(bucket.verifiedFraction)
                            // Keep a visible amber band whenever workers are active on this tile.
                            var rH = if (working) {
                                band(bucket.receivingFraction).coerceAtLeast(cell * 0.38f)
                            } else {
                                band(bucket.receivingFraction)
                            }
                            if (vH + rH > cell) {
                                val scale = cell / (vH + rH)
                                vH *= scale
                                rH *= scale
                            }
                            val remain = (cell - vH - rH).coerceAtLeast(0f)
                            val mH = if (bucket.missing > 0 || bucket.excluded > 0) {
                                remain * bucket.missingFraction /
                                    (bucket.missingFraction + bucket.excludedFraction).coerceAtLeast(0.0001f)
                            } else {
                                remain
                            }
                            val eH = (remain - mH).coerceAtLeast(0f)
                            val clip = Path().apply {
                                addRoundRect(
                                    RoundRect(
                                        left = left,
                                        top = top,
                                        right = left + cell,
                                        bottom = top + cell,
                                        radiusX = radius.x,
                                        radiusY = radius.y,
                                    ),
                                )
                            }
                            clipPath(clip) {
                                var yCursor = top
                                if (vH > 0f) {
                                    drawRect(
                                        color = verifiedColor,
                                        topLeft = Offset(left, yCursor),
                                        size = Size(cell, vH),
                                    )
                                    yCursor += vH
                                }
                                if (rH > 0f) {
                                    drawRect(
                                        color = receivingColor,
                                        topLeft = Offset(left, yCursor),
                                        size = Size(cell, rH),
                                    )
                                    yCursor += rH
                                }
                                if (mH > 0f) {
                                    drawRect(
                                        color = missingColor,
                                        topLeft = Offset(left, yCursor),
                                        size = Size(cell, mH),
                                    )
                                    yCursor += mH
                                }
                                if (eH > 0f) {
                                    drawRect(
                                        color = excludedFill,
                                        topLeft = Offset(left, yCursor),
                                        size = Size(cell, eH),
                                    )
                                }
                            }
                            when {
                                working -> drawRoundRect(
                                    color = receivingGlow,
                                    topLeft = origin,
                                    size = cellSize,
                                    cornerRadius = radius,
                                    style = activeStroke,
                                )
                                bucket.verifiedFraction < 1f || bucket.mixedFill || eH > 0f -> drawRoundRect(
                                    color = if (bucket.verifiedFraction >= 1f && !bucket.mixedFill) {
                                        verifiedColor.copy(alpha = 0.15f)
                                    } else {
                                        missingStroke
                                    },
                                    topLeft = origin,
                                    size = cellSize,
                                    cornerRadius = radius,
                                    style = stroke,
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

                if (markers.any { it.label != null }) {
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        markers.filter { it.label != null }.take(3).forEach { marker ->
                            Text(
                                marker.label!!,
                                style = MaterialTheme.typography.labelSmall,
                                color = colors.onSurfaceVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f, fill = false),
                            )
                        }
                    }
                }

                PieceHeatLegend(summary = summary)

                val peek = peekIndex?.let { idx -> buckets.getOrNull(idx)?.let { peekForBucket(it) } }
                if (peek != null) {
                    Surface(
                        shape = RoundedCornerShape(10.dp),
                        color = colors.secondaryContainer.copy(alpha = 0.65f),
                        contentColor = colors.onSecondaryContainer,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text(
                            peek.line(),
                            style = MaterialTheme.typography.labelSmall,
                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun PieceHeatLegend(summary: PieceHeatSummary) {
    val colors = MaterialTheme.colorScheme
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LegendPill(
            color = colors.primary,
            hollow = false,
            label = "Done",
            count = summary.selectedVerified,
            modifier = Modifier.weight(1f),
        )
        LegendPill(
            color = Color(0xFFFFB300),
            hollow = false,
            label = "Getting",
            count = summary.selectedReceiving,
            modifier = Modifier.weight(1f),
        )
        LegendPill(
            color = colors.outlineVariant,
            hollow = false,
            label = "Waiting",
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

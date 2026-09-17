package webtor.app

import webtor.core.PieceBucket
import webtor.core.PieceTelemetry
import kotlin.math.ceil
import kotlin.math.max

/** Disjoint selected-aware counts for one inclusive piece bucket. */
data class NormalizedPieceBucket(
    val index: Int,
    val start: Int,
    val end: Int,
    val total: Int,
    val selected: Int,
    val selectedVerified: Int,
    val selectedReceiving: Int,
    val missing: Int,
    val excluded: Int,
) {
    val verifiedFraction: Float get() = fraction(selectedVerified)
    val receivingFraction: Float get() = fraction(selectedReceiving)
    val missingFraction: Float get() = fraction(missing)
    val excludedFraction: Float get() = fraction(excluded)

    /** True when the bucket has no selected pieces (hollow tile). */
    val excludedOnly: Boolean get() = selected == 0 && total > 0

    /**
     * Mixed when any non-verified selected work remains or excluded shares the cell.
     * Fully selected-verified with no excluded paints solid; never treat a mixed
     * bucket as fully verified.
     */
    val mixedFill: Boolean
        get() = total > 0 && !excludedOnly &&
            (selectedReceiving > 0 || missing > 0 || excluded > 0 || selectedVerified < selected)

    private fun fraction(count: Int): Float =
        if (total <= 0) 0f else (count.toFloat() / total.toFloat()).coerceIn(0f, 1f)
}

data class PieceHeatSummary(
    val totalPieces: Int,
    val pieceLength: Long,
    val selected: Int,
    val selectedVerified: Int,
    val selectedReceiving: Int,
    val missing: Int,
    val excluded: Int,
) {
    /** Selected verified percent, or null when nothing is selected. */
    val verifiedPercent: Int?
        get() = if (selected <= 0) null
        else ((100.0 * selectedVerified) / selected).toInt().coerceIn(0, 100)

    fun titlePrimary(): String = when {
        selected <= 0 -> "No selected pieces"
        else -> "${verifiedPercent}% verified · $selectedReceiving receiving"
    }

    fun titleMeta(): String {
        val pieces = totalPieces.toString()
        return if (pieceLength > 0L) "$pieces · ${formatBytes(pieceLength)}" else pieces
    }
}

data class PieceHeatLayout(
    val columns: Int,
    val rows: Int,
    val cellCount: Int,
    /** Approximate cell edge in dp for the chosen density band. */
    val cellDp: Float,
    val gapDp: Float,
) {
    val heightDp: Float
        get() = if (rows <= 0) 0f else rows * cellDp + max(0, rows - 1) * gapDp
}

data class FileBoundaryMarker(
    val bucketIndex: Int,
    /** Short label for at most a few markers; null means tick-only. */
    val label: String?,
    /** True when more than one file starts inside the same piece. */
    val shared: Boolean,
)

data class PieceHeatPeek(
    val bucketIndex: Int,
    val start: Int,
    val end: Int,
    val selectedVerified: Int,
    val selectedReceiving: Int,
    val missing: Int,
    val excluded: Int,
) {
    fun line(): String =
        "Bucket $bucketIndex · pieces $start–$end · $selectedVerified verified · " +
            "$selectedReceiving receiving · $missing missing" +
            if (excluded > 0) " · $excluded excluded" else ""
}

fun normalizePieceBucket(
    index: Int,
    start: Int,
    end: Int,
    total: Int,
    selected: Int,
    selectedVerified: Int,
    selectedReceiving: Int,
): NormalizedPieceBucket {
    val safeTotal = total.coerceAtLeast(0)
    val safeSelected = selected.coerceIn(0, safeTotal)
    val safeVerified = selectedVerified.coerceIn(0, safeSelected)
    val safeReceiving = selectedReceiving.coerceIn(0, safeSelected - safeVerified)
    val missing = safeSelected - safeVerified - safeReceiving
    val excluded = safeTotal - safeSelected
    return NormalizedPieceBucket(
        index = index,
        start = start,
        end = end.coerceAtLeast(start),
        total = safeTotal,
        selected = safeSelected,
        selectedVerified = safeVerified,
        selectedReceiving = safeReceiving,
        missing = missing,
        excluded = excluded,
    )
}

fun normalizePieceBucket(index: Int, bucket: PieceBucket): NormalizedPieceBucket =
    normalizePieceBucket(
        index = index,
        start = bucket.start,
        end = bucket.end,
        total = bucket.total,
        selected = bucket.selected,
        selectedVerified = bucket.selectedVerified,
        selectedReceiving = bucket.selectedReceiving,
    )

fun normalizePieceBuckets(map: PieceTelemetry): List<NormalizedPieceBucket> =
    map.buckets.take(MAX_PIECE_HEAT_CELLS).mapIndexed { index, bucket ->
        normalizePieceBucket(index, bucket)
    }

fun summarizePieceHeat(map: PieceTelemetry, buckets: List<NormalizedPieceBucket> = normalizePieceBuckets(map)): PieceHeatSummary {
    var selected = 0
    var selectedVerified = 0
    var selectedReceiving = 0
    var missing = 0
    var excluded = 0
    for (b in buckets) {
        selected += b.selected
        selectedVerified += b.selectedVerified
        selectedReceiving += b.selectedReceiving
        missing += b.missing
        excluded += b.excluded
    }
    return PieceHeatSummary(
        totalPieces = map.totalPieces.coerceAtLeast(0),
        pieceLength = map.pieceLength.coerceAtLeast(0L),
        selected = selected,
        selectedVerified = selectedVerified,
        selectedReceiving = selectedReceiving,
        missing = missing,
        excluded = excluded,
    )
}

fun pieceHeatLayout(bucketCount: Int): PieceHeatLayout {
    val count = bucketCount.coerceIn(0, MAX_PIECE_HEAT_CELLS)
    // Ultra-dense map: more cubes in the same card width.
    val columns = when {
        count <= 0 -> 1
        count <= 64 -> 16
        count <= 128 -> 24
        else -> 32
    }
    val cellDp = when {
        count <= 64 -> 8f
        count <= 128 -> 6.5f
        else -> 5f
    }
    val gapDp = when {
        count <= 64 -> 0.9f
        count <= 128 -> 0.8f
        else -> 0.75f
    }
    val rows = if (count == 0) 0 else ceil(count / columns.toDouble()).toInt()
    return PieceHeatLayout(
        columns = columns,
        rows = rows,
        cellCount = count,
        cellDp = cellDp,
        gapDp = gapDp,
    )
}

/**
 * Height for a measured width so every cell stays square and the row fills
 * the full width (no empty strip on the right).
 */
fun pieceHeatHeightDp(layout: PieceHeatLayout, widthPx: Float, density: Float): Float {
    if (layout.rows <= 0 || widthPx <= 0f || density <= 0f) return layout.heightDp
    val widthDp = widthPx / density
    val gapsX = (layout.columns - 1).coerceAtLeast(0) * layout.gapDp
    val gapsY = (layout.rows - 1).coerceAtLeast(0) * layout.gapDp
    val cell = ((widthDp - gapsX) / layout.columns).coerceAtLeast(6f)
    return layout.rows * cell + gapsY
}

/**
 * Square cell edge sized to fill [width] exactly.
 * Height is expected to match [pieceHeatHeightDp]; unused [height] is ignored
 * so a max-height clamp can never leave a right-side gap.
 */
fun pieceHeatSquareCell(
    width: Float,
    height: Float,
    layout: PieceHeatLayout,
): Pair<Float, Float> {
    if (layout.cellCount <= 0 || width <= 0f || layout.columns <= 0) return 0f to 0f
    val cols = layout.columns
    val unit = cols * layout.cellDp + (cols - 1) * layout.gapDp
    val scale = width / unit.coerceAtLeast(0.001f)
    val gap = layout.gapDp * scale
    val cell = (width - gap * (cols - 1).coerceAtLeast(0)) / cols
    return cell.coerceAtLeast(0f) to gap
}

/**
 * Map a local canvas tap to a bucket index, or null when outside any cell.
 * Cells are squares packed from the top-left.
 */
fun hitTestPieceCell(
    x: Float,
    y: Float,
    width: Float,
    height: Float,
    layout: PieceHeatLayout,
): Int? {
    if (layout.cellCount <= 0 || width <= 0f || height <= 0f) return null
    if (x < 0f || y < 0f || x > width || y > height) return null
    val cols = layout.columns
    val rows = layout.rows
    val (cell, gap) = pieceHeatSquareCell(width, height, layout)
    if (cell <= 0f) return null
    val col = (x / (cell + gap)).toInt()
    val row = (y / (cell + gap)).toInt()
    if (col !in 0 until cols || row !in 0 until rows) return null
    val localX = x - col * (cell + gap)
    val localY = y - row * (cell + gap)
    if (localX > cell || localY > cell) return null
    val index = row * cols + col
    return if (index in 0 until layout.cellCount) index else null
}

/**
 * File-start markers from sequential file lengths and piece length.
 * A marker is the bucket containing the piece where that file begins.
 * Shared piece starts get honest combined wording; only 2–3 labels are kept.
 */
fun fileBoundaryMarkers(
    files: List<SavedFile>,
    pieceLength: Long,
    buckets: List<NormalizedPieceBucket>,
    maxLabels: Int = 3,
): List<FileBoundaryMarker> {
    if (files.isEmpty() || pieceLength <= 0L || buckets.isEmpty()) return emptyList()
    val ordered = files.sortedBy { it.index }
    // Group display names by the piece index where each file starts.
    val starts = LinkedHashMap<Int, MutableList<String>>()
    var offset = 0L
    for (file in ordered) {
        val pieceIndex = (offset / pieceLength).toInt()
        val name = shortFileLabel(file.name)
        starts.getOrPut(pieceIndex) { mutableListOf() }.add(name)
        offset += file.length.coerceAtLeast(0L)
    }
    if (starts.isEmpty()) return emptyList()

    val byBucket = LinkedHashMap<Int, MutableList<String>>()
    for ((pieceIndex, names) in starts) {
        val bucketIndex = buckets.indexOfFirst { pieceIndex in it.start..it.end }
        if (bucketIndex < 0) continue
        byBucket.getOrPut(bucketIndex) { mutableListOf() }.addAll(names)
    }

    val markers = byBucket.entries.map { (bucketIndex, names) ->
        val distinct = names.distinct()
        val shared = distinct.size > 1
        FileBoundaryMarker(
            bucketIndex = bucketIndex,
            label = null, // assigned below
            shared = shared,
        ) to distinct
    }

    // Prefer early buckets for visible labels; remainder stay tick-only.
    val labelBudget = maxLabels.coerceAtLeast(0)
    return markers.mapIndexed { order, (marker, names) ->
        val label = if (order < labelBudget) {
            when {
                names.size > 1 -> {
                    val head = names.take(2).joinToString(" / ")
                    val extra = if (names.size > 2) " +" else ""
                    "$head$extra (shared)"
                }
                else -> names.firstOrNull()
            }
        } else null
        marker.copy(label = label)
    }
}

fun peekForBucket(bucket: NormalizedPieceBucket): PieceHeatPeek =
    PieceHeatPeek(
        bucketIndex = bucket.index,
        start = bucket.start,
        end = bucket.end,
        selectedVerified = bucket.selectedVerified,
        selectedReceiving = bucket.selectedReceiving,
        missing = bucket.missing,
        excluded = bucket.excluded,
    )

fun accessibilityPieceHeatSummary(summary: PieceHeatSummary): String {
    val head = when (val pct = summary.verifiedPercent) {
        null -> "Pieces map. No selected pieces."
        else -> "Pieces map. $pct percent verified. ${summary.selectedReceiving} receiving."
    }
    return "$head ${summary.selectedVerified} verified, ${summary.selectedReceiving} receiving, " +
        "${summary.missing} missing, ${summary.excluded} excluded. " +
        "${summary.totalPieces} pieces" +
        if (summary.pieceLength > 0L) ", ${formatBytes(summary.pieceLength)} each." else "."
}

internal fun shortFileLabel(name: String, maxLen: Int = 10): String {
    val base = name.substringAfterLast('/').substringAfterLast('\\').ifBlank { name }
    if (base.length <= maxLen) return base
    return base.take(maxLen - 1) + "…"
}

const val MAX_PIECE_HEAT_CELLS = 256
/** Soft guide only; height follows square width-fill and is not clamped. */
const val MAX_PIECE_HEAT_HEIGHT_DP = 320f

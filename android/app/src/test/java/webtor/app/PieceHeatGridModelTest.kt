package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PieceHeatGridModelTest {

    @Test
    fun normalizeClampsSelectedAndDisjointCounts() {
        val b = normalizePieceBucket(
            index = 0,
            start = 0,
            end = 7,
            total = 8,
            selected = 20, // over total
            selectedVerified = 9, // over selected clamp
            selectedReceiving = 5,
        )
        assertEquals(8, b.total)
        assertEquals(8, b.selected)
        assertEquals(8, b.selectedVerified)
        assertEquals(0, b.selectedReceiving)
        assertEquals(0, b.missing)
        assertEquals(0, b.excluded)
        assertFalse(b.mixedFill)
        assertFalse(b.excludedOnly)
    }

    @Test
    fun normalizeMixedFractionsDivideByTotalNotSelected() {
        val b = normalizePieceBucket(
            index = 2,
            start = 16,
            end = 23,
            total = 8,
            selected = 6,
            selectedVerified = 3,
            selectedReceiving = 2,
        )
        assertEquals(1, b.missing)
        assertEquals(2, b.excluded)
        assertEquals(0.375f, b.verifiedFraction, 0.0001f) // 3/8
        assertEquals(0.25f, b.receivingFraction, 0.0001f) // 2/8
        assertEquals(0.125f, b.missingFraction, 0.0001f) // 1/8
        assertEquals(0.25f, b.excludedFraction, 0.0001f) // 2/8
        assertTrue(b.mixedFill)
        val sum = b.verifiedFraction + b.receivingFraction + b.missingFraction + b.excludedFraction
        assertEquals(1f, sum, 0.0001f)
    }

    @Test
    fun excludedOnlyStaysHollowAndReceivingClampsToRemaining() {
        val excluded = normalizePieceBucket(0, 0, 3, total = 4, selected = 0, selectedVerified = 2, selectedReceiving = 1)
        assertTrue(excluded.excludedOnly)
        assertEquals(0, excluded.selectedVerified)
        assertEquals(0, excluded.selectedReceiving)
        assertEquals(4, excluded.excluded)
        assertEquals(1f, excluded.excludedFraction, 0.0001f)

        val partial = normalizePieceBucket(1, 4, 7, total = 4, selected = 3, selectedVerified = 2, selectedReceiving = 9)
        assertEquals(2, partial.selectedVerified)
        assertEquals(1, partial.selectedReceiving)
        assertEquals(0, partial.missing)
        assertEquals(1, partial.excluded)
        assertTrue(partial.mixedFill)
    }

    @Test
    fun summaryTitleUsesSelectedPercentAndZeroSelectedMessage() {
        val withSelected = PieceHeatSummary(
            totalPieces = 100,
            pieceLength = 262144,
            selected = 50,
            selectedVerified = 33,
            selectedReceiving = 4,
            missing = 13,
            excluded = 50,
        )
        assertEquals(66, withSelected.verifiedPercent)
        assertEquals("66% verified · 4 receiving", withSelected.titlePrimary())
        assertTrue(withSelected.titleMeta().contains("100"))
        assertTrue(withSelected.titleMeta().contains("KB") || withSelected.titleMeta().contains("MB"))

        val none = PieceHeatSummary(12, 4096, 0, 0, 0, 0, 12)
        assertNull(none.verifiedPercent)
        assertEquals("No selected pieces", none.titlePrimary())
    }

    @Test
    fun layoutBandsForSmallAndLargeBucketCounts() {
        val tiny = pieceHeatLayout(12)
        assertEquals(16, tiny.columns)
        assertEquals(1, tiny.rows)
        assertTrue(tiny.cellDp <= 8f)
        assertTrue(tiny.heightDp > 0f)

        val mid = pieceHeatLayout(100)
        assertEquals(24, mid.columns)
        assertEquals(5, mid.rows)

        val large = pieceHeatLayout(256)
        assertEquals(32, large.columns)
        assertEquals(8, large.rows)
        assertEquals(256, large.cellCount)
        assertEquals(5f, large.cellDp, 0.01f)

        val capped = pieceHeatLayout(999)
        assertEquals(32, capped.columns)
        assertEquals(MAX_PIECE_HEAT_CELLS, capped.cellCount)
        assertEquals(0, pieceHeatLayout(0).cellCount)
    }

    @Test
    fun heightFillsWidthWithSquares() {
        val layout = pieceHeatLayout(64)
        val narrow = pieceHeatHeightDp(layout, widthPx = 200f, density = 2f)
        val wide = pieceHeatHeightDp(layout, widthPx = 1200f, density = 2f)
        assertTrue(narrow > 0f)
        assertTrue(wide > narrow)
        // Width-based square sizing: cell * cols + gaps == widthDp.
        val (cell, gap) = pieceHeatSquareCell(400f, narrow, layout)
        val filled = layout.columns * cell + (layout.columns - 1) * gap
        assertEquals(400f, filled, 0.5f)
        assertTrue(wide >= layout.cellDp)
    }

    @Test
    fun hitTestMapsInclusiveCellsAndMissesGaps() {
        val layout = pieceHeatLayout(8) // ultra-dense small band: 16 cols × 1 row
        assertEquals(16, layout.columns)
        assertEquals(1, layout.rows)
        val custom = PieceHeatLayout(columns = 4, rows = 2, cellCount = 8, cellDp = 10f, gapDp = 2f)
        val w = 4 * 10f + 3 * 2f // 46
        val h = 2 * 10f + 1 * 2f // 22

        assertEquals(0, hitTestPieceCell(1f, 1f, w, h, custom))
        assertEquals(3, hitTestPieceCell(40f, 1f, w, h, custom)) // last col of row 0
        assertEquals(4, hitTestPieceCell(1f, 13f, w, h, custom)) // row 1
        assertEquals(7, hitTestPieceCell(40f, 13f, w, h, custom))

        // Gap between col 0 and 1 (x in (10, 12)).
        assertNull(hitTestPieceCell(11f, 1f, w, h, custom))
        assertNull(hitTestPieceCell(-1f, 1f, w, h, custom))
        assertNull(hitTestPieceCell(1f, 100f, w, h, custom))
        // Beyond last populated cell when cellCount < rows*cols.
        val sparse = PieceHeatLayout(4, 2, cellCount = 5, cellDp = 10f, gapDp = 2f)
        assertEquals(4, hitTestPieceCell(1f, 13f, w, h, sparse))
        assertNull(hitTestPieceCell(40f, 13f, w, h, sparse))
    }

    @Test
    fun fileBoundariesMarkStartsSharedPieceAndLimitLabels() {
        // One piece per bucket so piece index == bucket index.
        val buckets = (0 until 8).map { i ->
            normalizePieceBucket(i, i, i, total = 1, selected = 1, selectedVerified = 0, selectedReceiving = 0)
        }
        // Offsets: 0, 150, 200, 250, 251 → pieces 0,1,2,2,2 with pieceLength=100.
        val files = listOf(
            SavedFile(0, "alpha.mkv", "alpha.mkv", 150),
            SavedFile(1, "beta.mkv", "beta.mkv", 50),
            SavedFile(2, "gamma.mkv", "gamma.mkv", 50),
            SavedFile(3, "delta.mkv", "delta.mkv", 1),
            SavedFile(4, "epsilon.mkv", "epsilon.mkv", 400),
        )
        val limited = fileBoundaryMarkers(files, pieceLength = 100, buckets = buckets, maxLabels = 2)
        assertEquals(listOf(0, 1, 2), limited.map { it.bucketIndex })
        assertEquals("alpha.mkv", limited[0].label)
        assertEquals("beta.mkv", limited[1].label)
        assertTrue(limited[2].shared)
        assertNull(limited[2].label) // label budget exhausted; tick only

        val labeledShared = fileBoundaryMarkers(files, pieceLength = 100, buckets = buckets, maxLabels = 3)
        assertTrue(labeledShared[2].shared)
        assertTrue(labeledShared[2].label!!.contains("shared"))
        assertTrue(labeledShared[2].label!!.contains("gamma"))
    }

    @Test
    fun peekLineUsesInclusiveRange() {
        val b = normalizePieceBucket(7, 56, 63, 8, 8, 3, 2)
        val peek = peekForBucket(b)
        assertEquals("Bucket 7 · pieces 56–63 · 3 verified · 2 receiving · 3 missing", peek.line())
    }

    @Test
    fun accessibilityMentionsEmptySelection() {
        val text = accessibilityPieceHeatSummary(
            PieceHeatSummary(10, 1024, selected = 0, selectedVerified = 0, selectedReceiving = 0, missing = 0, excluded = 10),
        )
        assertTrue(text.contains("No selected pieces"))
        assertTrue(text.contains("excluded"))
    }
}

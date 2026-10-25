package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import webtor.core.PlaybackDetail
import webtor.core.PlaybackPieceRange

class PlaybackBufferModelTest {
    private val ranges = listOf(
        PlaybackPieceRange(8, 9, "B"),
        PlaybackPieceRange(10, 11, "U"),
        PlaybackPieceRange(12, 15, "N"),
        PlaybackPieceRange(16, 21, "R"),
        PlaybackPieceRange(22, 23, "L"),
    )

    @Test
    fun oneTilePerPieceWithItsTierPlayheadAndReadiness() {
        // Pieces 8..23: rewind ready, playhead and next ready, then a gap.
        val detail = PlaybackDetail(8, "vvvsvrmmvvmmmmmm", 10)
        val model = playbackBufferModel(detail, ranges)
        assertEquals(16, model.tiles.size)
        assertEquals(1, model.piecesPerTile)
        assertEquals(listOf("B", "B", "U", "U", "N", "N"), model.tiles.take(6).map { it.tier })
        assertTrue(model.tiles[2].playhead)
        assertEquals(1, model.tiles.count { it.playhead })
        assertTrue("s is saved on storage", model.tiles[3].savedOnly)
        assertEquals(1, model.tiles[5].receiving)
        // 10 v, 11 s, 12 v ready without a gap; 13 is arriving.
        assertEquals(3, model.readyAhead)
        assertEquals(2, model.readyBehind)
        assertEquals(
            listOf(
                PlaybackTierCount("U", 2, 2), PlaybackTierCount("N", 1, 4), PlaybackTierCount("R", 2, 6),
                PlaybackTierCount("L", 0, 2), PlaybackTierCount("B", 2, 2),
            ),
            model.tiers,
        )
    }

    @Test
    fun longWindowsGroupEvenlyAndKeepTheMajorityTier() {
        val long = listOf(PlaybackPieceRange(0, 0, "U"), PlaybackPieceRange(1, 999, "R"))
        val model = playbackBufferModel(PlaybackDetail(0, "v".repeat(1000), 0), long, maxTiles = 100)
        assertEquals(10, model.piecesPerTile)
        assertEquals(100, model.tiles.size)
        // One urgent piece among ten readahead pieces does not paint the tile urgent.
        assertEquals("R", model.tiles[0].tier)
        assertTrue(model.tiles[0].playhead)
        assertEquals(1000, model.readyAhead)
    }

    @Test
    fun aCellTakesTheTierCoveringMostOfItsPieces() {
        val bucket = normalizePieceBucket(0, 0, 63, 64, 64, 0, 0)
        val bands = playbackBands(bucket, listOf(PlaybackPieceRange(10, 10, "U"), PlaybackPieceRange(11, 63, "R")))
        assertEquals("R", dominantPlaybackTier(bands))
        val tie = playbackBands(bucket, listOf(PlaybackPieceRange(0, 31, "B"), PlaybackPieceRange(32, 63, "N")))
        assertEquals("N", dominantPlaybackTier(tie))
        assertEquals("U", playbackTierAt(10, listOf(PlaybackPieceRange(0, 20, "R"), PlaybackPieceRange(10, 10, "U"))))
    }

    @Test
    fun ramSliderSpansSixteenMiBToOneGiB() {
        assertEquals(16, RAM_CACHE_STEPS_MIB.first())
        assertEquals(1024, RAM_CACHE_STEPS_MIB.last())
        assertEquals(RAM_CACHE_STEPS_MIB, RAM_CACHE_STEPS_MIB.sorted())
        assertEquals(64, RAM_CACHE_STEPS_MIB[ramCacheStepIndex(64)])
        assertEquals(96, RAM_CACHE_STEPS_MIB[ramCacheStepIndex(100)])
        assertEquals("1 GiB", formatRamCacheMiB(1024))
        assertEquals("48 MiB", formatRamCacheMiB(48))
        assertEquals(16, RAM_CACHE_STEPS_MIB[ramCacheStepIndex(1)])
        assertFalse(RAM_CACHE_STEPS_MIB.any { it < 16 || it > 1024 })
    }
}

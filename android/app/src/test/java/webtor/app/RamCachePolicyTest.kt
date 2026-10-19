package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Test

class RamCachePolicyTest {
    private val gib = 1024L * 1024L * 1024L

    @Test fun lowRamDevicesKeepTheMinimumBudget() {
        assertEquals(64, autoRamCacheMiB(8 * gib, 6 * gib, gib / 4, true, 1))
    }

    @Test fun automaticBudgetSharesMemoryAcrossTransfers() {
        assertEquals(512, autoRamCacheMiB(8 * gib, 4 * gib, gib / 4, false, 1))
        assertEquals(256, autoRamCacheMiB(8 * gib, 4 * gib, gib / 4, false, 2))
        assertEquals(128, autoRamCacheMiB(8 * gib, 4 * gib, gib / 4, false, 4))
    }

    @Test fun availableMemoryCanReduceTheDeviceBudget() {
        assertEquals(128, autoRamCacheMiB(8 * gib, gib + gib / 2, gib / 4, false, 1))
        assertEquals(64, autoRamCacheMiB(8 * gib, gib / 2, gib / 4, false, 1))
    }

    @Test fun systemLowMemoryThresholdIsReserved() {
        assertEquals(64, autoRamCacheMiB(8 * gib, 2 * gib, 2 * gib, false, 1))
    }

    @Test fun idleAndInvalidInputsRemainBounded() {
        assertEquals(512, autoRamCacheMiB(16 * gib, 10 * gib, gib / 4, false, 0))
        assertEquals(64, autoRamCacheMiB(0, 0, 0, false, -1))
        assertEquals(64, autoRamCacheMiB(8 * gib, -1, -1, false, 1))
    }
}

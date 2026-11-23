package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Test
import webtor.app.Policy.settings as s

/**
 * The app reads its defaults and tuning from shared/policy.json (generated into [Policy]).
 * These are the values the app had hard-coded before; a change here is a behaviour change
 * for both apps, so update this test only on purpose.
 */
class PolicyTest {
    @Test fun settingDefaultsMatchTheOldLiterals() {
        assertEquals(false, s.directPlayDefault.default)
        assertEquals(15, s.idleDropMinutes.default)
        assertEquals("HTTP", s.playbackMethod.default)
        assertEquals(true, s.builtInPlayer.default)
        assertEquals(false, s.rememberBrightness.default)
        assertEquals("ANY", s.networkPolicy.default)
        assertEquals(0, s.downloadRateKiB.default)
        assertEquals(55, s.maxPeers.default)
        assertEquals(false, s.antiDpiEnabled.default)
        assertEquals(true, s.seedingEnabled.default)
        assertEquals(1000, s.seedRatioMilli.default)
        assertEquals(60, s.seedTimeMinutes.default)
        assertEquals(256, s.seedUploadKiB.default)
        assertEquals(true, s.seedWiFiOnly.default)
        assertEquals(false, s.seedChargingOnly.default)
        assertEquals(false, s.pauseOnLowBattery.default)
        assertEquals(15, s.pauseOnLowBattery.batteryPercent)
        assertEquals(true, s.ramCacheAuto.default)
        assertEquals(64, s.ramCacheMiB.default)
        assertEquals(true, s.darkTheme.default)
        assertEquals("AMOLED", s.appTheme.default)
        assertEquals(false, s.dynamicColors.default)
        assertEquals(false, s.reducedEffects.default)
    }

    @Test fun rangesMatchTheOldLiterals() {
        assertEquals(0 to 240, s.idleDropMinutes.min to s.idleDropMinutes.max)
        assertEquals(8 to 500, s.maxPeers.min to s.maxPeers.max)
        assertEquals(100 to 10000, s.seedRatioMilli.min to s.seedRatioMilli.max)
        assertEquals(1 to 1440, s.seedTimeMinutes.min to s.seedTimeMinutes.max)
        assertEquals(16 to 4096, s.seedUploadKiB.min to s.seedUploadKiB.max)
    }

    @Test fun choicesMatchTheOldLiterals() {
        assertEquals(listOf(true to "Built-in", false to "External app"), s.builtInPlayer.options.zip(s.builtInPlayer.optionLabels))
        assertEquals(listOf(false to "Download", true to "Direct Play"), s.directPlayDefault.options.zip(s.directPlayDefault.optionLabels))
        assertEquals(listOf(0 to "Off", 5 to "5 min", 15 to "15 min", 30 to "30 min"), s.idleDropMinutes.options.zip(s.idleDropMinutes.optionLabels))
        assertEquals(listOf("HTTP" to "Stream", "CONTENT_URI" to "File proxy"), s.playbackMethod.options.zip(s.playbackMethod.optionLabels))
        assertEquals(
            listOf("ANY" to "Any network", "WIFI_ONLY" to "Wi-Fi", "UNMETERED" to "Unmetered"),
            s.networkPolicy.options.zip(s.networkPolicy.optionLabels),
        )
        assertEquals(listOf(0 to "None", 1024 to "1 MB/s", 4096 to "4 MB/s", 8192 to "8 MB/s"), s.downloadRateKiB.options.zip(s.downloadRateKiB.optionLabels))
        assertEquals(listOf(500 to "0.5×", 1000 to "1×", 2000 to "2×"), s.seedRatioMilli.options.zip(s.seedRatioMilli.optionLabels))
        assertEquals(listOf(15 to "15 min", 60 to "1 hour", 180 to "3 hours"), s.seedTimeMinutes.options.zip(s.seedTimeMinutes.optionLabels))
        assertEquals(listOf(128 to "128 KiB/s", 256 to "256 KiB/s", 512 to "512 KiB/s"), s.seedUploadKiB.options.zip(s.seedUploadKiB.optionLabels))
        assertEquals(
            listOf("AMOLED" to "AMOLED black", "AERO" to "Aero", "ANTIQUITY" to "Antiquity", "RETRO" to "Retro", "WADA" to "Wada"),
            s.appTheme.options.zip(s.appTheme.optionLabels),
        )
        // Every option names a real enum value.
        s.playbackMethod.options.forEach { PlaybackMethod.valueOf(it) }
        s.networkPolicy.options.forEach { TransferNetworkPolicy.valueOf(it) }
        s.appTheme.options.forEach { AppTheme.valueOf(it) }
    }

    @Test fun ramCacheMatchesTheOldLiterals() {
        assertEquals(listOf(16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024), Policy.ramCache.stepsMiB)
        val auto = Policy.ramCache.auto
        assertEquals(10, auto.reserveTotalDivisor)
        assertEquals(16, auto.budgetTotalDivisor)
        assertEquals(4, auto.budgetAvailableDivisor)
        assertEquals(listOf(128, 64), auto.autoStepsMiB)
        assertEquals(64, auto.minMiB)
        assertEquals(128, auto.maxMiB)
    }

    @Test fun playAndEngineMatchTheOldLiterals() {
        val p = Policy.play
        assertEquals(listOf(true, true, true, false, false, true), listOf(p.boost, p.fileOnly, p.adaptiveBuffer, p.burstAwareBuffer, p.preloadStart, p.playNext))
        assertEquals(5, p.playNextCountdownSeconds)
        val e = Policy.engine
        assertEquals(false, e.ghostMode)
        assertEquals(true, e.ghostProMode)
        assertEquals(true, e.claudeSeekMode)
        assertEquals(true, e.hedgeRequests)
        assertEquals(true, e.slowPeerYield)
        assertEquals(true, e.dropChokers)
        assertEquals(false, e.deadlineSeek)
        assertEquals(false, e.peerExploration)
        assertEquals(false, e.fastPeerSearch)
        assertEquals(30, e.dialRatePerSec)
        assertEquals(80, e.fastPeerSearchDialRatePerSec)
    }

    @Test fun uiStateDefaultsComeFromThePolicy() {
        val state = UiState()
        assertEquals(55, state.maxPeers)
        assertEquals(64, state.ramCacheMiB)
        assertEquals(AppTheme.AMOLED, state.appTheme)
        assertEquals(PlaybackMethod.HTTP, state.playbackMethod)
        assertEquals(TransferNetworkPolicy.ANY, state.networkPolicy)
        assertEquals(0, state.downloadRateKiB)
    }
}

package webtor.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ThemeTest {
    @Test
    fun parseSavedThemeDefaultsToAeroAndHandlesUnknown() {
        assertEquals(AppTheme.AERO, parseSavedTheme(null))
        assertEquals(AppTheme.AERO, parseSavedTheme(""))
        assertEquals(AppTheme.AERO, parseSavedTheme("unknown_theme"))
        assertEquals(AppTheme.AERO, parseSavedTheme("AERO"))
        assertEquals(AppTheme.ANTIQUITY, parseSavedTheme("antiquity"))
        assertEquals(AppTheme.RETRO, parseSavedTheme("Retro"))
        assertEquals(AppTheme.WADA, parseSavedTheme("wada"))
    }

    @Test
    fun threeThemesHaveDistinctColorPalettes() {
        val aeroLight = themeColorScheme(AppTheme.AERO, dark = false)
        val antiquityLight = themeColorScheme(AppTheme.ANTIQUITY, dark = false)
        val retroLight = themeColorScheme(AppTheme.RETRO, dark = false)

        assertNotEquals(aeroLight.primary, antiquityLight.primary)
        assertNotEquals(aeroLight.primary, retroLight.primary)
        assertNotEquals(antiquityLight.primary, retroLight.primary)

        assertNotEquals(aeroLight.background, antiquityLight.background)
        assertNotEquals(aeroLight.background, retroLight.background)
        assertNotEquals(antiquityLight.background, retroLight.background)
    }

    @Test
    fun wadaUsesCombination166AndSetsItsOwnSurfaceContainers() {
        val light = themeColorScheme(AppTheme.WADA, dark = false)
        val dark = themeColorScheme(AppTheme.WADA, dark = true)
        val deepSlateGreen = androidx.compose.ui.graphics.Color(0xFF112F2C)
        val grenadinePink = androidx.compose.ui.graphics.Color(0xFFF48067)
        assertEquals(deepSlateGreen, light.primary)
        assertEquals(grenadinePink, dark.primary)
        assertEquals(androidx.compose.ui.graphics.Color(0xFFFBE6A0), dark.secondary) // Naples Yellow
        assertEquals(deepSlateGreen, dark.surfaceContainer)
        AppTheme.entries.filter { it != AppTheme.WADA }.forEach {
            assertNotEquals(themeColorScheme(it, dark = true).primary, dark.primary)
        }
    }

    @Test
    fun wadaPieceColorsAvoidThePrimaryAndOthersKeepDefaults() {
        for (dark in listOf(false, true)) {
            val scheme = themeColorScheme(AppTheme.WADA, dark)
            val pieces = pieceColors(AppTheme.WADA, dark, dynamicColors = false)
            assertNotEquals(scheme.primary, pieces.urgent)
            val tiles = listOf(pieces.done, pieces.getting, pieces.urgent, pieces.readahead, pieces.lookahead)
            assertEquals(tiles.size, tiles.toSet().size)
        }
        assertEquals(DefaultPieceColors, pieceColors(AppTheme.WADA, dark = true, dynamicColors = true))
        assertEquals(DefaultPieceColors, pieceColors(AppTheme.AERO, dark = true, dynamicColors = false))
    }

    @Test
    fun eachThemeHasDistinctLightAndDarkAppearance() {
        AppTheme.entries.forEach { theme ->
            val light = themeColorScheme(theme, dark = false)
            val dark = themeColorScheme(theme, dark = true)
            assertNotEquals(light.background, dark.background)
            assertNotEquals(light.surface, dark.surface)
        }
    }

    @Test fun allPaletteSurfacesAreOpaque() {
        for (theme in AppTheme.entries) for (dark in listOf(false, true)) {
            val colors = themeColorScheme(theme, dark)
            assertEquals(1f, colors.surface.alpha)
            assertEquals(1f, colors.background.alpha)
        }
        assertTrue(UiState().dynamicColors)
    }
}

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

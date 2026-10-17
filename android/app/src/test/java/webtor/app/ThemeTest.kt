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

    @Test
    fun decorationCacheProvidesPrecomputedArtwork() {
        val aeroDec = getThemeDecoration(AppTheme.AERO, dark = true)
        val antiquityDec = getThemeDecoration(AppTheme.ANTIQUITY, dark = false)
        val retroDec = getThemeDecoration(AppTheme.RETRO, dark = true)

        assertTrue(aeroDec.hasGlassTreatment)
        assertTrue(antiquityDec.hasOrnamentTreatment)
        assertTrue(retroDec.hasBevelTreatment)

        // Cached lookup returns same instance
        val aeroDec2 = getThemeDecoration(AppTheme.AERO, dark = true)
        assertTrue(aeroDec === aeroDec2)
    }
}

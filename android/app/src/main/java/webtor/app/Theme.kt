package webtor.app

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Surface
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowInsetsControllerCompat
import java.util.concurrent.ConcurrentHashMap

val LocalAppTheme = compositionLocalOf { AppTheme.AERO }
val LocalReducedEffects = compositionLocalOf { false }

// --- 1. AERO THEME (Vista/Windows 7 inspired glass & gloss) ---
private val AeroLightColors = lightColorScheme(
    primary = Color(0xFF0078D7),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFD6EEFF),
    onPrimaryContainer = Color(0xFF002244),
    secondary = Color(0xFF009688),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFD0F0EC),
    onSecondaryContainer = Color(0xFF002420),
    tertiary = Color(0xFF0284C7),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFBAE6FD),
    onTertiaryContainer = Color(0xFF082F49),
    background = Color(0xFFF2F7FC),
    onBackground = Color(0xFF131D24),
    surface = Color(0xFFEAF3FB),
    onSurface = Color(0xFF131D24),
    surfaceVariant = Color(0xFFD5E5F2),
    onSurfaceVariant = Color(0xFF384954),
    outline = Color(0xFF6B8294),
    outlineVariant = Color(0xFFBED2E0),
    error = Color(0xFFBA1A1A),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF410002),
)

private val AeroDarkColors = darkColorScheme(
    primary = Color(0xFF38BDF8),
    onPrimary = Color(0xFF00283B),
    primaryContainer = Color(0xFF0E3A52),
    onPrimaryContainer = Color(0xFFBEE9FF),
    secondary = Color(0xFF4FD1C5),
    onSecondary = Color(0xFF003833),
    secondaryContainer = Color(0xFF1A4540),
    onSecondaryContainer = Color(0xFF99F6E4),
    tertiary = Color(0xFF7DD3FC),
    onTertiary = Color(0xFF00354C),
    tertiaryContainer = Color(0xFF0C4A6E),
    onTertiaryContainer = Color(0xFFE0F2FE),
    background = Color(0xFF0B141C),
    onBackground = Color(0xFFE1EBF2),
    surface = Color(0xFF111E28),
    onSurface = Color(0xFFE1EBF2),
    surfaceVariant = Color(0xFF1D2E3C),
    onSurfaceVariant = Color(0xFFB2C6D4),
    outline = Color(0xFF748F9E),
    outlineVariant = Color(0xFF263D4F),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
)

// --- 2. ANTIQUITY THEME (Art Nouveau, parchment, brass & botanicals) ---
private val AntiquityLightColors = lightColorScheme(
    primary = Color(0xFF8B5A2B), // warm brass/leather
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFF5E6CC),
    onPrimaryContainer = Color(0xFF2D1B06),
    secondary = Color(0xFF5A7253), // botanical sage
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFDCEAD7),
    onSecondaryContainer = Color(0xFF162512),
    tertiary = Color(0xFFB8860B), // celestial gold
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFFCE9B8),
    onTertiaryContainer = Color(0xFF382800),
    background = Color(0xFFFAF6EE), // warm ivory
    onBackground = Color(0xFF26211C),
    surface = Color(0xFFF5EFE3), // parchment
    onSurface = Color(0xFF26211C),
    surfaceVariant = Color(0xFFE9DEC9),
    onSurfaceVariant = Color(0xFF52483E),
    outline = Color(0xFF857767),
    outlineVariant = Color(0xFFD4C5B0),
    error = Color(0xFF9E2A2B),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFD9D9),
    onErrorContainer = Color(0xFF400507),
)

private val AntiquityDarkColors = darkColorScheme(
    primary = Color(0xFFD4A054), // illuminated gold
    onPrimary = Color(0xFF331F04),
    primaryContainer = Color(0xFF4A3415),
    onPrimaryContainer = Color(0xFFFCE3BE),
    secondary = Color(0xFF9AB492), // muted botanical
    onSecondary = Color(0xFF142910),
    secondaryContainer = Color(0xFF2E3F29),
    onSecondaryContainer = Color(0xFFDCEAD7),
    tertiary = Color(0xFFECC46F),
    onTertiary = Color(0xFF3D2C00),
    tertiaryContainer = Color(0xFF574108),
    onTertiaryContainer = Color(0xFFFCE9B8),
    background = Color(0xFF141210), // deep ink
    onBackground = Color(0xFFEBE4D8),
    surface = Color(0xFF1C1917), // velvet manuscript
    onSurface = Color(0xFFEBE4D8),
    surfaceVariant = Color(0xFF2C2520),
    onSurfaceVariant = Color(0xFFCEC2B2),
    outline = Color(0xFF938676),
    outlineVariant = Color(0xFF453B33),
    error = Color(0xFFF87171),
    onError = Color(0xFF4C080A),
    errorContainer = Color(0xFF7A1C1E),
    onErrorContainer = Color(0xFFFFD9D9),
)

// --- 3. RETRO THEME (1980s/1990s beveled desktop & pixel aesthetic) ---
private val RetroLightColors = lightColorScheme(
    primary = Color(0xFF000080), // classic window navy
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFB4C8E0),
    onPrimaryContainer = Color(0xFF000826),
    secondary = Color(0xFF2B5B84),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFD0E0EE),
    onSecondaryContainer = Color(0xFF051D30),
    tertiary = Color(0xFF008080), // classic teal
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFC0E0E0),
    onTertiaryContainer = Color(0xFF002828),
    background = Color(0xFFECE9D8), // classic desktop
    onBackground = Color(0xFF000000),
    surface = Color(0xFFD4D0C8), // classic OS 3D face grey
    onSurface = Color(0xFF000000),
    surfaceVariant = Color(0xFFBFBBB2),
    onSurfaceVariant = Color(0xFF2B2B2B),
    outline = Color(0xFF808080),
    outlineVariant = Color(0xFFFFFFFF),
    error = Color(0xFF800000),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFC0C0),
    onErrorContainer = Color(0xFF400000),
)

private val RetroDarkColors = darkColorScheme(
    primary = Color(0xFF00FFAA), // phosphor terminal cyan/green
    onPrimary = Color(0xFF002416),
    primaryContainer = Color(0xFF084530),
    onPrimaryContainer = Color(0xFFA6FFD7),
    secondary = Color(0xFF56B6C2),
    onSecondary = Color(0xFF0B2529),
    secondaryContainer = Color(0xFF1E3A42),
    onSecondaryContainer = Color(0xFFC4F0F7),
    tertiary = Color(0xFF61AFEF),
    onTertiary = Color(0xFF0A2238),
    tertiaryContainer = Color(0xFF1A3854),
    onTertiaryContainer = Color(0xFFCCE4FF),
    background = Color(0xFF181B20),
    onBackground = Color(0xFFD7DAE0),
    surface = Color(0xFF22262E),
    onSurface = Color(0xFFD7DAE0),
    surfaceVariant = Color(0xFF2D323C),
    onSurfaceVariant = Color(0xFFABB2BF),
    outline = Color(0xFF5C6370),
    outlineVariant = Color(0xFF3B4048),
    error = Color(0xFFE06C75),
    onError = Color(0xFF38080C),
    errorContainer = Color(0xFF5A141A),
    onErrorContainer = Color(0xFFFFD2D6),
)

fun themeColorScheme(theme: AppTheme, dark: Boolean): ColorScheme = when (theme) {
    AppTheme.AERO -> if (dark) AeroDarkColors else AeroLightColors
    AppTheme.ANTIQUITY -> if (dark) AntiquityDarkColors else AntiquityLightColors
    AppTheme.RETRO -> if (dark) RetroDarkColors else RetroLightColors
}

private val AeroShapes = Shapes(
    extraSmall = RoundedCornerShape(6.dp),
    small = RoundedCornerShape(10.dp),
    medium = RoundedCornerShape(14.dp),
    large = RoundedCornerShape(18.dp),
    extraLarge = RoundedCornerShape(26.dp),
)

private val AntiquityShapes = Shapes(
    extraSmall = RoundedCornerShape(4.dp),
    small = RoundedCornerShape(6.dp),
    medium = RoundedCornerShape(10.dp),
    large = RoundedCornerShape(14.dp),
    extraLarge = RoundedCornerShape(20.dp),
)

private val RetroShapes = Shapes(
    extraSmall = RoundedCornerShape(1.dp),
    small = RoundedCornerShape(2.dp),
    medium = RoundedCornerShape(3.dp),
    large = RoundedCornerShape(4.dp),
    extraLarge = RoundedCornerShape(6.dp),
)

fun themeShapes(theme: AppTheme): Shapes = when (theme) {
    AppTheme.AERO -> AeroShapes
    AppTheme.ANTIQUITY -> AntiquityShapes
    AppTheme.RETRO -> RetroShapes
}

fun themeTypography(theme: AppTheme): Typography {
    val headingFont = when (theme) {
        AppTheme.AERO -> FontFamily.SansSerif
        AppTheme.ANTIQUITY -> FontFamily.Serif
        AppTheme.RETRO -> FontFamily.Monospace
    }
    val bodyFont = when (theme) {
        AppTheme.RETRO -> FontFamily.Monospace
        else -> FontFamily.SansSerif
    }

    return Typography(
        displayLarge = TextStyle(fontFamily = headingFont, fontSize = 54.sp, lineHeight = 60.sp, fontWeight = FontWeight.Bold),
        displayMedium = TextStyle(fontFamily = headingFont, fontSize = 42.sp, lineHeight = 48.sp, fontWeight = FontWeight.Bold),
        displaySmall = TextStyle(fontFamily = headingFont, fontSize = 34.sp, lineHeight = 40.sp, fontWeight = FontWeight.SemiBold),
        headlineLarge = TextStyle(fontFamily = headingFont, fontSize = 30.sp, lineHeight = 36.sp, fontWeight = FontWeight.SemiBold),
        headlineMedium = TextStyle(fontFamily = headingFont, fontSize = 24.sp, lineHeight = 28.sp, fontWeight = FontWeight.SemiBold),
        headlineSmall = TextStyle(fontFamily = headingFont, fontSize = 20.sp, lineHeight = 24.sp, fontWeight = FontWeight.Medium),
        titleLarge = TextStyle(fontFamily = headingFont, fontSize = 18.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold),
        titleMedium = TextStyle(fontFamily = headingFont, fontSize = 16.sp, lineHeight = 22.sp, fontWeight = FontWeight.Medium),
        titleSmall = TextStyle(fontFamily = headingFont, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium),
        bodyLarge = TextStyle(fontFamily = bodyFont, fontSize = 15.sp, lineHeight = 22.sp),
        bodyMedium = TextStyle(fontFamily = bodyFont, fontSize = 13.sp, lineHeight = 19.sp),
        bodySmall = TextStyle(fontFamily = bodyFont, fontSize = 12.sp, lineHeight = 16.sp),
        labelLarge = TextStyle(fontFamily = bodyFont, fontSize = 14.sp, fontWeight = FontWeight.Medium),
        labelMedium = TextStyle(fontFamily = bodyFont, fontSize = 12.sp, fontWeight = FontWeight.Medium),
        labelSmall = TextStyle(fontFamily = bodyFont, fontSize = 11.sp, fontWeight = FontWeight.Normal),
    )
}

// CACHE-06: Precomputed static theme decoration cache
data class ThemeDecoration(
    val hasGlassTreatment: Boolean,
    val hasOrnamentTreatment: Boolean,
    val hasBevelTreatment: Boolean,
    val glassGlowBrush: Brush?,
    val glassBorderColor: Color,
    val brassOrnamentColor: Color,
    val bevelLightColor: Color,
    val bevelDarkColor: Color,
)

private val decorationCache = ConcurrentHashMap<Pair<AppTheme, Boolean>, ThemeDecoration>()

fun getThemeDecoration(theme: AppTheme, dark: Boolean): ThemeDecoration {
    return decorationCache.getOrPut(theme to dark) {
        when (theme) {
            AppTheme.AERO -> ThemeDecoration(
                hasGlassTreatment = true,
                hasOrnamentTreatment = false,
                hasBevelTreatment = false,
                glassGlowBrush = Brush.verticalGradient(
                    listOf(
                        if (dark) Color(0x3580D0FF) else Color(0x55FFFFFF),
                        Color(0x05FFFFFF),
                        Color(0x00FFFFFF),
                    )
                ),
                glassBorderColor = if (dark) Color(0x4038BDF8) else Color(0x600078D7),
                brassOrnamentColor = Color.Transparent,
                bevelLightColor = Color.Transparent,
                bevelDarkColor = Color.Transparent,
            )
            AppTheme.ANTIQUITY -> ThemeDecoration(
                hasGlassTreatment = false,
                hasOrnamentTreatment = true,
                hasBevelTreatment = false,
                glassGlowBrush = null,
                glassBorderColor = Color.Transparent,
                brassOrnamentColor = if (dark) Color(0xFFC59B27) else Color(0xFF8B5A2B),
                bevelLightColor = Color.Transparent,
                bevelDarkColor = Color.Transparent,
            )
            AppTheme.RETRO -> ThemeDecoration(
                hasGlassTreatment = false,
                hasOrnamentTreatment = false,
                hasBevelTreatment = true,
                glassGlowBrush = null,
                glassBorderColor = Color.Transparent,
                brassOrnamentColor = Color.Transparent,
                bevelLightColor = if (dark) Color(0xFF454C59) else Color(0xFFFFFFFF),
                bevelDarkColor = if (dark) Color(0xFF101216) else Color(0xFF808080),
            )
        }
    }
}

/**
 * Themed card container applying distinct glass, ornament, or bevel treatment.
 */
@Composable
fun ThemedCard(
    modifier: Modifier = Modifier,
    shape: Shape? = null,
    tonalElevation: Dp = 2.dp,
    content: @Composable BoxScope.() -> Unit,
) {
    val theme = LocalAppTheme.current
    val reducedEffects = LocalReducedEffects.current
    val dark = MaterialTheme.colorScheme.background.red < 0.5f
    val dec = getThemeDecoration(theme, dark)
    val cardShape = shape ?: themeShapes(theme).medium

    when {
        theme == AppTheme.AERO -> {
            Surface(
                modifier = modifier
                    .border(BorderStroke(1.dp, dec.glassBorderColor), cardShape)
                    .clip(cardShape),
                shape = cardShape,
                color = MaterialTheme.colorScheme.surface.copy(alpha = if (dark) 0.85f else 0.92f),
                tonalElevation = tonalElevation,
            ) {
                Box(
                    modifier = Modifier.then(
                        if (!reducedEffects && dec.glassGlowBrush != null) {
                            Modifier.background(dec.glassGlowBrush)
                        } else Modifier
                    )
                ) {
                    content()
                }
            }
        }
        theme == AppTheme.ANTIQUITY -> {
            Surface(
                modifier = modifier
                    .border(BorderStroke(1.5.dp, dec.brassOrnamentColor.copy(alpha = 0.5f)), cardShape)
                    .clip(cardShape),
                shape = cardShape,
                color = MaterialTheme.colorScheme.surface,
                tonalElevation = tonalElevation,
            ) {
                Box(
                    modifier = Modifier.drawBehind {
                        // Ornamental corner brackets in Antiquity
                        if (!reducedEffects) {
                            val strokeWidth = 1.5.dp.toPx()
                            val bracketLen = 14.dp.toPx()
                            val bracketColor = dec.brassOrnamentColor.copy(alpha = 0.6f)
                            // Top-left bracket
                            drawLine(bracketColor, Offset(4.dp.toPx(), 4.dp.toPx()), Offset(4.dp.toPx() + bracketLen, 4.dp.toPx()), strokeWidth)
                            drawLine(bracketColor, Offset(4.dp.toPx(), 4.dp.toPx()), Offset(4.dp.toPx(), 4.dp.toPx() + bracketLen), strokeWidth)
                            // Bottom-right bracket
                            drawLine(bracketColor, Offset(size.width - 4.dp.toPx() - bracketLen, size.height - 4.dp.toPx()), Offset(size.width - 4.dp.toPx(), size.height - 4.dp.toPx()), strokeWidth)
                            drawLine(bracketColor, Offset(size.width - 4.dp.toPx(), size.height - 4.dp.toPx() - bracketLen), Offset(size.width - 4.dp.toPx(), size.height - 4.dp.toPx()), strokeWidth)
                        }
                    }
                ) {
                    content()
                }
            }
        }
        theme == AppTheme.RETRO -> {
            // Classic 3D beveled container (light on top-left, dark shadow on bottom-right)
            Surface(
                modifier = modifier
                    .drawBehind {
                        val stroke = 2.dp.toPx()
                        // Top & Left highlight
                        drawLine(dec.bevelLightColor, Offset(0f, 0f), Offset(size.width, 0f), stroke)
                        drawLine(dec.bevelLightColor, Offset(0f, 0f), Offset(0f, size.height), stroke)
                        // Bottom & Right shadow
                        drawLine(dec.bevelDarkColor, Offset(0f, size.height), Offset(size.width, size.height), stroke)
                        drawLine(dec.bevelDarkColor, Offset(size.width, 0f), Offset(size.width, size.height), stroke)
                    }
                    .padding(2.dp),
                shape = cardShape,
                color = MaterialTheme.colorScheme.surface,
            ) {
                Box {
                    content()
                }
            }
        }
        else -> {
            Surface(
                modifier = modifier,
                shape = cardShape,
                color = MaterialTheme.colorScheme.surface,
                tonalElevation = tonalElevation,
            ) {
                Box { content() }
            }
        }
    }
}

/**
 * Screen container drawing theme-specific backdrops (watery depth for Aero,
 * parchment and botanical/celestial ornament for Antiquity, tactile desktop for Retro).
 */
@Composable
fun ThemedScreenContainer(
    modifier: Modifier = Modifier,
    content: @Composable BoxScope.() -> Unit,
) {
    val theme = LocalAppTheme.current
    val reducedEffects = LocalReducedEffects.current
    val dark = MaterialTheme.colorScheme.background.red < 0.5f

    Box(
        modifier = modifier
            .fillMaxSize()
            .drawBehind {
                when (theme) {
                    AppTheme.AERO -> {
                        val waterBrush = Brush.verticalGradient(
                            colors = if (dark) listOf(
                                Color(0xFF07111A),
                                Color(0xFF0E2233),
                                Color(0xFF091622),
                                Color(0xFF040A0F),
                            ) else listOf(
                                Color(0xFFE4F1FB),
                                Color(0xFFD0E7F9),
                                Color(0xFFE8F4FC),
                                Color(0xFFF7FBFE),
                            )
                        )
                        drawRect(brush = waterBrush)
                        if (!reducedEffects) {
                            val causticColor = if (dark) Color(0x1838BDF8) else Color(0x200078D7)
                            drawCircle(causticColor, radius = size.width * 0.75f, center = Offset(size.width * 0.2f, size.height * 0.15f))
                            drawCircle(causticColor, radius = size.width * 0.6f, center = Offset(size.width * 0.9f, size.height * 0.6f))
                        }
                    }
                    AppTheme.ANTIQUITY -> {
                        val parchmentBrush = Brush.verticalGradient(
                            colors = if (dark) listOf(
                                Color(0xFF161412),
                                Color(0xFF1A1714),
                                Color(0xFF141210),
                            ) else listOf(
                                Color(0xFFFBF8F1),
                                Color(0xFFF3ECE0),
                                Color(0xFFFAF5ED),
                            )
                        )
                        drawRect(brush = parchmentBrush)
                        if (!reducedEffects) {
                            val ornamentColor = if (dark) Color(0x15D4A054) else Color(0x188B5A2B)
                            drawCircle(ornamentColor, radius = 70.dp.toPx(), center = Offset(size.width - 40.dp.toPx(), 60.dp.toPx()))
                            drawCircle(ornamentColor, radius = 110.dp.toPx(), center = Offset(30.dp.toPx(), size.height - 100.dp.toPx()))
                        }
                    }
                    AppTheme.RETRO -> {
                        val retroBg = if (dark) Color(0xFF14171C) else Color(0xFFECE9D8)
                        drawRect(color = retroBg)
                        if (!reducedEffects && !dark) {
                            val stippleColor = Color(0x0C000000)
                            drawLine(stippleColor, Offset(0f, 0f), Offset(size.width, size.height), strokeWidth = 1f)
                        }
                    }
                }
            }
    ) {
        content()
    }
}

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}

@Composable
fun GaleTheme(
    dark: Boolean = true,
    theme: AppTheme = AppTheme.AERO,
    reducedEffects: Boolean = false,
    content: @Composable () -> Unit,
) {
    val colors = themeColorScheme(theme, dark)
    val shapes = themeShapes(theme)
    val typography = themeTypography(theme)

    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = view.context.activity()?.window ?: return@SideEffect
            val bar = colors.background.toArgb()
            window.statusBarColor = bar
            window.navigationBarColor = bar
            WindowInsetsControllerCompat(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }

    CompositionLocalProvider(
        LocalAppTheme provides theme,
        LocalReducedEffects provides reducedEffects,
    ) {
        MaterialTheme(
            colorScheme = colors,
            shapes = shapes,
            typography = typography,
            content = content,
        )
    }
}

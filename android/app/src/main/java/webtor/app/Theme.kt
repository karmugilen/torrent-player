package webtor.app

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowInsetsControllerCompat

val LocalAppTheme = compositionLocalOf { AppTheme.AERO }
val LocalReducedEffects = compositionLocalOf { false }

// --- 1. AERO THEME (optional cool blue palette) ---
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

// --- 2. ANTIQUITY THEME (optional warm palette) ---
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

// --- 3. RETRO THEME (optional classic palette) ---
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

// --- 4. WADA THEME ---
// Sanzo Wada, A Dictionary of Color Combinations (1933-34), combination 166:
// Grenadine Pink, Naples Yellow and Deep Slate Green. Combination 325 adds
// Yellow Ocher; other roles use nearby colors from the same book. Values from
// the MIT-licensed dictionary-of-colour-combinations dataset (Matt DesLauriers).
private val WadaLightColors = lightColorScheme(
    primary = Color(0xFF112F2C), // Deep Slate Green
    onPrimary = Color(0xFFF5ECC2), // Sulpher Yellow
    primaryContainer = Color(0xFFB5DECC), // Turquoise Green
    onPrimaryContainer = Color(0xFF0B1F1D),
    inversePrimary = Color(0xFFF48067), // Grenadine Pink
    secondary = Color(0xFFC55347), // Etruscan Red, a text-safe Grenadine
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFFDD4BD), // Seashell Pink
    onSecondaryContainer = Color(0xFF4A1810),
    tertiary = Color(0xFF986F2D), // Orange Citrine
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFFBE6A0), // Naples Yellow
    onTertiaryContainer = Color(0xFF3A2A06),
    background = Color(0xFFFAF4DD),
    onBackground = Color(0xFF112F2C),
    surface = Color(0xFFFAF4DD),
    onSurface = Color(0xFF112F2C),
    surfaceVariant = Color(0xFFEBD3A2), // Ivory Buff
    onSurfaceVariant = Color(0xFF4E4A34),
    surfaceTint = Color(0xFF112F2C),
    inverseSurface = Color(0xFF112F2C),
    inverseOnSurface = Color(0xFFF5ECC2),
    error = Color(0xFFA62C37), // Carmine Red
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFF8B6BA), // Corinthian Pink
    onErrorContainer = Color(0xFF410009),
    outline = Color(0xFF8B835B), // Dark Citrine
    outlineVariant = Color(0xFFD6C99C),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFFFFFBEA),
    surfaceContainerLowest = Color(0xFFFFFBEA),
    surfaceContainerLow = Color(0xFFF8F0D2),
    surfaceContainer = Color(0xFFF5ECC2), // Sulpher Yellow
    surfaceContainerHigh = Color(0xFFEFE3B3),
    surfaceContainerHighest = Color(0xFFEBD3A2), // Ivory Buff
    surfaceDim = Color(0xFFE3D9AE),
)

private val WadaDarkColors = darkColorScheme(
    primary = Color(0xFFF48067), // Grenadine Pink
    onPrimary = Color(0xFF112F2C), // Deep Slate Green
    primaryContainer = Color(0xFF5C2A20),
    onPrimaryContainer = Color(0xFFFDD4BD), // Seashell Pink
    inversePrimary = Color(0xFFC55347), // Etruscan Red
    secondary = Color(0xFFFBE6A0), // Naples Yellow
    onSecondary = Color(0xFF112F2C),
    secondaryContainer = Color(0xFF3E3A1F),
    onSecondaryContainer = Color(0xFFFBE6A0),
    tertiary = Color(0xFFE2B540), // Yellow Ocher
    onTertiary = Color(0xFF2B2105),
    tertiaryContainer = Color(0xFF4A3A0E),
    onTertiaryContainer = Color(0xFFFBE6A0),
    background = Color(0xFF0B1F1D),
    onBackground = Color(0xFFF5ECC2), // Sulpher Yellow
    surface = Color(0xFF0B1F1D),
    onSurface = Color(0xFFF5ECC2),
    surfaceVariant = Color(0xFF1E403C),
    onSurfaceVariant = Color(0xFFC8C2A0),
    surfaceTint = Color(0xFFF48067),
    inverseSurface = Color(0xFFF5ECC2),
    inverseOnSurface = Color(0xFF112F2C),
    error = Color(0xFFF8B6BA), // Corinthian Pink
    onError = Color(0xFF5A0F18),
    errorContainer = Color(0xFF7A1F2A),
    onErrorContainer = Color(0xFFF8B6BA),
    outline = Color(0xFFA2B0AD), // Mineral Gray
    outlineVariant = Color(0xFF34504C),
    scrim = Color(0xFF000000),
    surfaceBright = Color(0xFF2A4F4B),
    surfaceContainerLowest = Color(0xFF071614),
    surfaceContainerLow = Color(0xFF0F2A27),
    surfaceContainer = Color(0xFF112F2C), // Deep Slate Green
    surfaceContainerHigh = Color(0xFF1A3D3A),
    surfaceContainerHighest = Color(0xFF214643),
    surfaceDim = Color(0xFF0B1F1D),
)

fun themeColorScheme(theme: AppTheme, dark: Boolean): ColorScheme = when (theme) {
    AppTheme.AERO -> if (dark) AeroDarkColors else AeroLightColors
    AppTheme.ANTIQUITY -> if (dark) AntiquityDarkColors else AntiquityLightColors
    AppTheme.RETRO -> if (dark) RetroDarkColors else RetroLightColors
    AppTheme.WADA -> if (dark) WadaDarkColors else WadaLightColors
}

/**
 * Pieces grid colors. Null [done] means the scheme's primary. Wada uses book
 * colors so urgent never matches its Grenadine Pink primary.
 */
data class PieceColors(
    val done: Color?,
    val getting: Color,
    val gettingGlow: Color,
    val urgent: Color,
    val readahead: Color,
    val lookahead: Color,
)

val DefaultPieceColors = PieceColors(
    done = null,
    getting = Color(0xFFFFB300),
    gettingGlow = Color(0xFFFF6F00),
    urgent = Color(0xFFFF8A80),
    readahead = Color(0xFF82B1FF),
    lookahead = Color(0xFFCE93D8),
)

fun pieceColors(theme: AppTheme, dark: Boolean, dynamicColors: Boolean): PieceColors = when {
    dynamicColors || theme != AppTheme.WADA -> DefaultPieceColors
    dark -> PieceColors(
        done = Color(0xFFFBE6A0), // Naples Yellow
        getting = Color(0xFFF99D1B), // Yellow Orange
        gettingGlow = Color(0xFFF37420), // Orange
        urgent = Color(0xFFF37F94), // Eosine Pink
        readahead = Color(0xFF78CDD0), // Calamine Blue
        lookahead = Color(0xFFB984AF), // Lilac
    )
    else -> PieceColors(
        done = Color(0xFF112F2C), // Deep Slate Green
        getting = Color(0xFFC16B27), // Orange Rufous
        gettingGlow = Color(0xFFAE5224), // Burnt Sienna
        urgent = Color(0xFFDA525D), // Eugenia Red
        readahead = Color(0xFF00939B), // Peacock Blue
        lookahead = Color(0xFFA36AA5), // Aconite Violet
    )
}

val LocalPieceColors = compositionLocalOf { DefaultPieceColors }


/** Opaque Material surfaces are shared by the system and optional colour palettes. */
@Composable
fun ThemedCard(modifier: Modifier = Modifier, shape: Shape? = null,
    tonalElevation: Dp = 1.dp, content: @Composable BoxScope.() -> Unit) {
    Surface(modifier, shape = shape ?: MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.surfaceContainer, tonalElevation = tonalElevation) {
        Box(content = content)
    }
}

@Composable
fun ThemedScreenContainer(modifier: Modifier = Modifier, content: @Composable BoxScope.() -> Unit) {
    Surface(modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Box(Modifier.fillMaxSize(), content = content)
    }
}

@Composable
fun GaleTheme(dark: Boolean = true, theme: AppTheme = AppTheme.AERO,
    reducedEffects: Boolean = false, dynamicColors: Boolean = true, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val dynamic = dynamicColors && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
    val colors = if (dynamic) {
        if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    } else themeColorScheme(theme, dark)
    val view = LocalView.current
    if (!view.isInEditMode) SideEffect {
        view.context.activity()?.window?.let { window ->
            window.statusBarColor = colors.background.toArgb()
            window.navigationBarColor = colors.background.toArgb()
            WindowInsetsControllerCompat(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    CompositionLocalProvider(
        LocalAppTheme provides theme,
        LocalReducedEffects provides reducedEffects,
        LocalPieceColors provides pieceColors(theme, dark, dynamic),
    ) {
        MaterialTheme(colorScheme = colors, shapes = Shapes(), typography = Typography(), content = content)
    }
}

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}

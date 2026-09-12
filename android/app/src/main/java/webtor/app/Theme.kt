package webtor.app

import android.app.Activity
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowInsetsControllerCompat

private val LightInk = Color(0xFF2F3437)
private val LightBone = Color(0xFFF7F6F3)
private val LightPaper = Color(0xFFFBFBFA)
private val LightLine = Color(0xFFEAEAEA)
private val LightMute = Color(0xFF787774)
private val LightGreenBg = Color(0xFFEDF3EC)
private val LightGreen = Color(0xFF346538)
private val LightRedBg = Color(0xFFFDEBEC)
private val LightRed = Color(0xFF9F2F2D)
private val LightYellowBg = Color(0xFFFBF3DB)
private val LightYellow = Color(0xFF956400)
private val LightCharcoal = Color(0xFF111111)

private val Night = Color(0xFF161513)
private val NightPaper = Color(0xFF1C1B19)
private val NightInk = Color(0xFFF4F1EA)
private val NightMute = Color(0xFF9A958C)
private val NightLine = Color(0xFF2E2C29)
private val NightGreenBg = Color(0xFF243028)
private val NightGreen = Color(0xFFB7CDB4)
private val NightRedBg = Color(0xFF3A2424)
private val NightRed = Color(0xFFE0A8A6)
private val NightYellowBg = Color(0xFF3A3220)
private val NightYellow = Color(0xFFD4B56A)

private val LightGaleColors = lightColorScheme(
    primary = LightCharcoal,
    onPrimary = LightPaper,
    secondary = LightGreen,
    onSecondary = LightGreenBg,
    background = LightBone,
    onBackground = LightInk,
    surface = LightPaper,
    onSurface = LightInk,
    surfaceVariant = Color(0xFFF9F9F8),
    onSurfaceVariant = LightMute,
    error = LightRed,
    onError = LightRedBg,
    errorContainer = LightRedBg,
    onErrorContainer = LightRed,
    outline = LightLine,
    primaryContainer = LightGreenBg,
    onPrimaryContainer = LightGreen,
    tertiary = LightYellow,
    tertiaryContainer = LightYellowBg,
    onTertiaryContainer = LightYellow,
)

private val DarkGaleColors = darkColorScheme(
    primary = NightInk,
    onPrimary = Night,
    secondary = NightGreen,
    onSecondary = NightGreenBg,
    background = Night,
    onBackground = NightInk,
    surface = NightPaper,
    onSurface = NightInk,
    surfaceVariant = Color(0xFF22211E),
    onSurfaceVariant = NightMute,
    error = NightRed,
    onError = NightRedBg,
    errorContainer = NightRedBg,
    onErrorContainer = NightRed,
    outline = NightLine,
    primaryContainer = NightGreenBg,
    onPrimaryContainer = NightGreen,
    tertiary = NightYellow,
    tertiaryContainer = NightYellowBg,
    onTertiaryContainer = NightYellow,
)

private val GaleShapes = Shapes(
    extraSmall = RoundedCornerShape(4.dp),
    small = RoundedCornerShape(6.dp),
    medium = RoundedCornerShape(8.dp),
    large = RoundedCornerShape(12.dp),
)

private val GaleTypography = Typography(
    headlineLarge = TextStyle(
        fontFamily = FontFamily.Serif,
        fontWeight = FontWeight.Medium,
        fontSize = 32.sp,
        lineHeight = 36.sp,
        letterSpacing = (-0.6).sp,
    ),
    headlineMedium = TextStyle(
        fontFamily = FontFamily.Serif,
        fontWeight = FontWeight.Medium,
        fontSize = 26.sp,
        lineHeight = 30.sp,
        letterSpacing = (-0.4).sp,
    ),
    headlineSmall = TextStyle(
        fontFamily = FontFamily.Serif,
        fontWeight = FontWeight.Medium,
        fontSize = 22.sp,
        lineHeight = 26.sp,
        letterSpacing = (-0.3).sp,
    ),
    titleLarge = TextStyle(
        fontFamily = FontFamily.Serif,
        fontWeight = FontWeight.Medium,
        fontSize = 20.sp,
        lineHeight = 24.sp,
        letterSpacing = (-0.2).sp,
    ),
    titleMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Medium,
        fontSize = 16.sp,
        lineHeight = 22.sp,
        letterSpacing = (-0.2).sp,
    ),
    bodyLarge = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 16.sp,
        lineHeight = 25.sp,
    ),
    bodyMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 14.sp,
        lineHeight = 22.sp,
    ),
    bodySmall = TextStyle(
        fontFamily = FontFamily.Monospace,
        fontWeight = FontWeight.Normal,
        fontSize = 12.sp,
        lineHeight = 18.sp,
    ),
    labelLarge = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Medium,
        fontSize = 14.sp,
        letterSpacing = 0.2.sp,
    ),
    labelSmall = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Medium,
        fontSize = 11.sp,
        letterSpacing = 0.8.sp,
    ),
)

@Composable
fun GaleTheme(dark: Boolean = true, content: @Composable () -> Unit) {
    val colors = if (dark) DarkGaleColors else LightGaleColors
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as Activity).window
            val bar = colors.background.toArgb()
            window.statusBarColor = bar
            window.navigationBarColor = bar
            WindowInsetsControllerCompat(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    MaterialTheme(
        colorScheme = colors,
        shapes = GaleShapes,
        typography = GaleTypography,
        content = content,
    )
}

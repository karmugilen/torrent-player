package webtor.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density

/**
 * YouTube-style compact look for the home and details pages: everything
 * (text, buttons, gaps) drawn about 12% smaller. Other screens keep full size.
 */
@Composable
fun CompactDensity(content: @Composable () -> Unit) {
    val d = LocalDensity.current
    CompositionLocalProvider(LocalDensity provides Density(d.density * 0.9f, d.fontScale * 0.97f)) {
        content()
    }
}

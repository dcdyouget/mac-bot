package bot.mac.mobile.core.design

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density

/**
 * Restricts system font scaling inside [content] while leaving layout density untouched.
 *
 * Implementing Density directly is intentional: delegating Density methods would also delegate
 * their text conversion to the uncapped system density. Compose's platform FontScaling defaults
 * continue to convert SP through the platform converter using this density's capped fontScale.
 */
@Composable
fun FontScaleLimit(maxFontScale: Float, content: @Composable () -> Unit) {
    require(maxFontScale.isFinite() && maxFontScale > 0f) { "maxFontScale must be finite and positive" }
    val systemDensity = LocalDensity.current
    val limitedDensity = remember(systemDensity, maxFontScale) {
        object : Density {
            override val density: Float = systemDensity.density
            override val fontScale: Float = systemDensity.fontScale.coerceAtMost(maxFontScale)
        }
    }
    CompositionLocalProvider(LocalDensity provides limitedDensity, content = content)
}

@Composable
fun FontScaleToolbar(content: @Composable () -> Unit) = FontScaleLimit(STANDARD_FONT_SCALE_LIMIT, content)

const val STANDARD_FONT_SCALE_LIMIT = 1.3f
const val MESSAGE_FONT_SCALE_LIMIT = 2.0f

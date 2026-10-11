package bot.mac.mobile.core.ui

import androidx.compose.foundation.border
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.runtime.Composable
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.DesignElevation
import bot.mac.mobile.core.design.DesignTokens
import bot.mac.mobile.core.design.ElevationToken
import bot.mac.mobile.core.design.LocalMobilePalette

/** Applies the token's semantic level with Compose's single ambient shadow and token border. */
@Composable
fun Modifier.designElevation(token: ElevationToken, shape: Shape): Modifier {
    val palette = LocalMobilePalette.current
    val shadowColor = palette.tokenColors["shadow.color"] ?: DesignTokens.shadowColor
    val borderColor = token.borderToken?.let { palette.tokenColors[it] } ?: Color.Transparent
    var result = this
    if (token.blur > 0.dp) result = result.shadow(token.blur, shape, ambientColor = shadowColor, spotColor = shadowColor)
    if (token.borderWidth > 0.dp) result = result.border(token.borderWidth, borderColor, shape)
    return result
}

/** Common card preset; it intentionally follows Compose's one-shadow model. */
@Composable
fun Modifier.designCardElevation(shape: Shape): Modifier = designElevation(DesignElevation.level1, shape)

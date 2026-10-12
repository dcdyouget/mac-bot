package bot.mac.mobile.feature.chat

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.LocalMobilePalette
import kotlin.math.PI

@Composable
internal fun ChatTypingWave(animationsEnabled: Boolean) {
    val tint = LocalMobilePalette.current.textSecondary
    val transition = if (animationsEnabled) rememberInfiniteTransition(label = "typingWave") else null
    Row(
        Modifier.padding(vertical = 3.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        repeat(3) { index ->
            val phaseState = transition?.animateFloat(
                initialValue = 0f,
                targetValue = 1f,
                animationSpec = infiniteRepeatable(
                    animation = tween(durationMillis = 640, delayMillis = index * 150),
                    repeatMode = RepeatMode.Restart,
                ),
                label = "typingDot$index",
            ) ?: remember { mutableFloatStateOf(0f) }
            val phase by phaseState
            val lift = if (animationsEnabled) (kotlin.math.sin(phase * 2f * PI).toFloat().coerceAtLeast(0f) * 3f).dp else 0.dp
            Box(Modifier.offset(y = -lift).size(5.dp).background(tint, RoundedCornerShape(50)))
        }
    }
}

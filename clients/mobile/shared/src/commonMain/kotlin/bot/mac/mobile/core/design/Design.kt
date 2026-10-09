package bot.mac.mobile.core.design

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.size
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

object DesignTokens {
    val accent = Color(0xFF2F7BF6)
    val success = Color(0xFF34C759)
    val attention = Color(0xFFFF9F0A)
    val danger = Color(0xFFFF3B30)
    val space = listOf(4, 8, 12, 16, 24)
    val bubbleRadius = 18.dp
    val cardRadius = 14.dp
    val inputRadius = 22.dp
    val beanColors = listOf(0xFF7B74E8,0xFF3885E5,0xFF39B997,0xFFFFAD4F,0xFFE66887,0xFFBC73D6,0xFF68B6D6,0xFF7B9653,0xFFD48157,0xFF8B8C9D).map(::Color)
}
private val LightColors = lightColorScheme(
    primary = DesignTokens.accent, onPrimary = Color.White, background = Color.White,
    surface = Color.White, surfaceContainer = Color(0xFFF5F5F7), surfaceVariant = Color(0xFFEFEFF1),
    onBackground = Color(0xFF1D1D1F), onSurface = Color(0xFF1D1D1F), onSurfaceVariant = Color(0xFF86868B),
    primaryContainer = Color(0xFF111111), onPrimaryContainer = Color.White, error = DesignTokens.danger,
)
private val DarkColors = darkColorScheme(
    primary = Color(0xFF4C8DFF), onPrimary = Color.White, background = Color(0xFF1C1C1E),
    surface = Color(0xFF1C1C1E), surfaceContainer = Color(0xFF232325), surfaceVariant = Color(0xFF2C2C2E),
    onBackground = Color(0xFFF5F5F7), onSurface = Color(0xFFF5F5F7), onSurfaceVariant = Color(0xFF98989D),
    primaryContainer = Color(0xFFF2F2F2), onPrimaryContainer = Color.Black, error = DesignTokens.danger,
)
@Composable fun MacBotTheme(mode: String, content: @Composable () -> Unit) {
    val dark = mode == "dark" || mode == "system" && isSystemInDarkTheme()
    MaterialTheme(colorScheme = if (dark) DarkColors else LightColors,
        typography = Typography(
            bodyLarge = TextStyle(fontSize = 14.sp, lineHeight = 21.sp),
            bodyMedium = TextStyle(fontSize = 14.sp, lineHeight = 20.sp),
            bodySmall = TextStyle(fontSize = 12.sp, lineHeight = 18.sp),
            titleMedium = TextStyle(fontSize = 15.sp, fontWeight = FontWeight.SemiBold),
            titleLarge = TextStyle(fontSize = 18.sp, fontWeight = FontWeight.SemiBold),
            labelSmall = TextStyle(fontSize = 12.sp), labelMedium = TextStyle(fontSize = 12.sp),
        ), content = content)
}
@Composable fun BeanAvatar(color: Int, main: Boolean = false, status: String = "idle") {
    Canvas(Modifier.size(42.dp)) {
        val radius = size.minDimension * .42f
        drawCircle(DesignTokens.beanColors[color.mod(10)], radius, center)
        for (x in listOf(.4f, .6f)) drawCircle(Color.White, 3.4.dp.toPx(), Offset(size.width*x,size.height*.5f))
        for (x in listOf(.4f, .6f)) drawCircle(Color(0xFF222222), 1.6.dp.toPx(), Offset(size.width*x,size.height*.5f))
        if (main) {
            val crown = androidx.compose.ui.graphics.Path().apply {
                moveTo(size.width*.29f,size.height*.18f); lineTo(size.width*.29f,size.height*.02f)
                lineTo(size.width*.42f,size.height*.10f); lineTo(size.width*.5f,0f)
                lineTo(size.width*.58f,size.height*.10f); lineTo(size.width*.71f,size.height*.02f)
                lineTo(size.width*.71f,size.height*.18f); close()
            }; drawPath(crown, Color(0xFFFFC04A))
        }
        if (status != "idle") drawCircle(when(status) { "waiting_user" -> DesignTokens.attention; "blocked" -> DesignTokens.danger; "done" -> DesignTokens.success; else -> DesignTokens.accent }, 4.dp.toPx(), Offset(size.width*.82f,size.height*.82f))
    }
}

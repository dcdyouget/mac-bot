package bot.mac.mobile.core.design

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.size
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Values are generated from design/tokens.json. Keep token names aligned across clients. */
object DesignTokens {
    val bgDesktop = Color(0xFFE9E8E4)
    val bgWindow = Color(0xFFFBFBFA)
    val bgSidebar = Color(0xFFF3F3F1)
    val bgPanel = Color(0xFFF7F7F5)
    val bgChat = Color(0xFFEFEFEB)
    val bgElevated = Color(0xFFFFFFFF)
    val bgInput = Color(0xFFFFFFFF)
    val bgHover = Color(0x0B141418)
    val bgActive = Color(0x13141418)
    val bgSelected = Color(0x1F4A6A9B)
    val bgOverlay = Color(0x47141418)
    val borderSubtle = Color(0x0F141418)
    val borderDefault = Color(0x1A141418)
    val borderStrong = Color(0x2E141418)
    val textPrimary = Color(0xFF1C1C1E)
    val textSecondary = Color(0xFF6E6E73)
    val textTertiary = Color(0xFFA1A1A6)
    val textInverse = Color(0xFFFFFFFF)
    val textLink = Color(0xFF3F5F92)
    val accent = Color(0xFF4A6A9B)
    val accentHover = Color(0xFF3F5D8A)
    val accentPressed = Color(0xFF355079)
    val accentSoft = Color(0x1F4A6A9B)
    val accentOn = Color(0xFFFFFFFF)
    val bubbleIn = Color(0xFFFFFFFF)
    val bubbleInText = Color(0xFF1C1C1E)
    val bubbleOut = Color(0xFFDDE5F0)
    val bubbleOutText = Color(0xFF1C1C1E)
    val bubbleMeta = Color(0xFF8E8E93)
    val bubbleOutMeta = Color(0xFF6F819C)
    val success = Color(0xFF3E8E66)
    val successSoft = Color(0x1F3E8E66)
    val warning = Color(0xFFB7812C)
    val warningSoft = Color(0x21B7812C)
    val danger = Color(0xFFB9533F)
    val dangerSoft = Color(0x1CB9533F)
    val review = Color(0xFF7B65A8)
    val reviewSoft = Color(0x1F7B65A8)
    val codeText = Color(0xFF9A4A5E)
    val codeBg = Color(0x0D141418)
    val chart1 = Color(0xFF4A6A9B)
    val chart2 = Color(0xFF5E9C88)
    val chart3 = Color(0xFFC0956A)
    val chart4 = Color(0xFF9A7BB5)
    val chart5 = Color(0xFFB86B6B)
    val chart6 = Color(0xFF8C8C8C)
    val heat0 = Color(0xFFECECE8)
    val heat1 = Color(0xFFCDD7E5)
    val heat2 = Color(0xFF9DB0CC)
    val heat3 = Color(0xFF6B87B0)
    val heat4 = Color(0xFF4A6A9B)
    val shadowColor = Color(0x1A141418)
    val dark = mapOf(
        "bg.desktop" to Color(0xFF0B0B0C),
        "bg.window" to Color(0xFF18181A),
        "bg.sidebar" to Color(0xFF141416),
        "bg.panel" to Color(0xFF1A1A1D),
        "bg.chat" to Color(0xFF111113),
        "bg.elevated" to Color(0xFF232326),
        "bg.input" to Color(0xFF1F1F22),
        "bg.hover" to Color(0x0DFFFFFF),
        "bg.active" to Color(0x14FFFFFF),
        "bg.selected" to Color(0x298AA6D6),
        "bg.overlay" to Color(0x80000000),
        "border.subtle" to Color(0x0FFFFFFF),
        "border.default" to Color(0x1AFFFFFF),
        "border.strong" to Color(0x2EFFFFFF),
        "text.primary" to Color(0xFFECECEE),
        "text.secondary" to Color(0xFF9A9AA0),
        "text.tertiary" to Color(0xFF66666B),
        "text.inverse" to Color(0xFF111113),
        "text.link" to Color(0xFF9DB5DE),
        "accent" to Color(0xFF8AA6D6),
        "accent.hover" to Color(0xFF9DB5DE),
        "accent.pressed" to Color(0xFF7A95C4),
        "accent.soft" to Color(0x298AA6D6),
        "accent.on" to Color(0xFF111113),
        "bubble.in" to Color(0xFF222225),
        "bubble.in.text" to Color(0xFFECECEE),
        "bubble.out" to Color(0xFF2B3A52),
        "bubble.out.text" to Color(0xFFECEFF5),
        "bubble.meta" to Color(0xFF7C7C82),
        "bubble.out.meta" to Color(0xFF8FA2C0),
        "success" to Color(0xFF6DBE94),
        "success.soft" to Color(0x246DBE94),
        "warning" to Color(0xFFDDAA5C),
        "warning.soft" to Color(0x24DDAA5C),
        "danger" to Color(0xFFE08470),
        "danger.soft" to Color(0x24E08470),
        "review" to Color(0xFFAE9BD6),
        "review.soft" to Color(0x26AE9BD6),
        "code.text" to Color(0xFFE3A3B3),
        "code.bg" to Color(0x0FFFFFFF),
        "chart.1" to Color(0xFF8AA6D6),
        "chart.2" to Color(0xFF7FC2AC),
        "chart.3" to Color(0xFFD9B08A),
        "chart.4" to Color(0xFFB9A0D4),
        "chart.5" to Color(0xFFD98F8F),
        "chart.6" to Color(0xFF8E8E93),
        "heat.0" to Color(0xFF1F1F22),
        "heat.1" to Color(0xFF26324A),
        "heat.2" to Color(0xFF3A5070),
        "heat.3" to Color(0xFF5D7BA8),
        "heat.4" to Color(0xFF8AA6D6),
        "shadow.color" to Color(0x73000000),
    )
    val translucentLight = mapOf(
        "bg.window" to Color(0xB8FBFBFA),
        "bg.sidebar" to Color(0x94F3F3F1),
        "bg.panel" to Color(0xA3F7F7F5),
        "bg.chat" to Color(0xDBEFEFEB),
        "bg.elevated" to Color(0xC7FFFFFF),
        "bg.input" to Color(0xCCFFFFFF),
    )
    val translucentDark = mapOf(
        "bg.window" to Color(0xB218181A),
        "bg.sidebar" to Color(0x8C141416),
        "bg.panel" to Color(0x9E1A1A1D),
        "bg.chat" to Color(0xDB111113),
        "bg.elevated" to Color(0xC7232326),
        "bg.input" to Color(0xCC1F1F22),
    )
    val space0 = 0.dp
    val space1 = 2.dp
    val space2 = 4.dp
    val space3 = 6.dp
    val space4 = 8.dp
    val space5 = 12.dp
    val space6 = 16.dp
    val space7 = 20.dp
    val space8 = 24.dp
    val space9 = 32.dp
    val space10 = 40.dp
    val space11 = 48.dp
    val space12 = 64.dp
    val radiusxs = 4.dp
    val radiussm = 6.dp
    val radiusmd = 8.dp
    val radiuslg = 12.dp
    val radiusxl = 16.dp
    val radius2xl = 20.dp
    val radiusbubble = 16.dp
    val radiusbubbleTail = 6.dp
    val radiuscard = 12.dp
    val radiussheet = 20.dp
    val radiuswindow = 12.dp
    val radiuspill = 999.dp
    val avatarColors = listOf(Color(0xFF7C93B8), Color(0xFF86A894), Color(0xFFC6A47E), Color(0xFFA08DBF), Color(0xFFC48A8A), Color(0xFF7FA9AE), Color(0xFFB5A36C), Color(0xFF9B9B9B), Color(0xFFC49AB0), Color(0xFF8C9BC4))
    val avatarEye = Color(0xFF1C1C1E)
    val avatarGlasses = Color(0xFF1C1C1E)
    val attention = warning
}

object MobileLayout {
    val statusBar = 24.dp
    val topAppBar = 64.dp
    val navigationBar = 80.dp
    val gestureBar = 24.dp
    val chatRow = 72.dp
    val listAvatar = 48.dp
    val messageAvatar = 32.dp
    val fab = 56.dp
    val minimumTouchTarget = 48.dp
    val sheetTopRadius = 28.dp
    val primaryButtonHeight = 48.dp
}

data class MobilePalette(
    val window: Color, val sidebar: Color, val panel: Color, val chat: Color,
    val elevated: Color, val input: Color, val textPrimary: Color, val textSecondary: Color,
    val textTertiary: Color, val accent: Color, val accentSoft: Color,
    val bubbleIn: Color, val bubbleInText: Color, val bubbleOut: Color, val bubbleOutText: Color,
    val success: Color, val successSoft: Color, val warning: Color, val warningSoft: Color,
    val danger: Color, val dangerSoft: Color, val review: Color, val reviewSoft: Color,
    val borderDefault: Color, val borderSubtle: Color, val codeBg: Color,
)
val LocalMobilePalette = compositionLocalOf {
    MobilePalette(Color(0xFFFBFBFA), Color(0xFFF3F3F1), Color(0xFFF7F7F5), Color(0xFFEFEFEB), Color(0xFFFFFFFF), Color(0xFFFFFFFF), Color(0xFF1C1C1E), Color(0xFF6E6E73), Color(0xFFA1A1A6), Color(0xFF4A6A9B), Color(0x1F4A6A9B), Color(0xFFFFFFFF), Color(0xFF1C1C1E), Color(0xFFDDE5F0), Color(0xFF1C1C1E), Color(0xFF3E8E66), Color(0x1F3E8E66), Color(0xFFB7812C), Color(0x21B7812C), Color(0xFFB9533F), Color(0x1CB9533F), Color(0xFF7B65A8), Color(0x1F7B65A8), Color(0x1A141418), Color(0x0F141418), Color(0x0D141418))
}

private fun palette(dark: Boolean, translucent: Boolean): MobilePalette {
    val base = if (dark) DesignTokens.dark else emptyMap()
    val overlay = if (translucent) if (dark) DesignTokens.translucentDark else DesignTokens.translucentLight else emptyMap()
    fun value(key: String, light: Color): Color = overlay[key] ?: base[key] ?: light
    return MobilePalette(
        value("bg.window", DesignTokens.bgWindow), value("bg.sidebar", DesignTokens.bgSidebar), value("bg.panel", DesignTokens.bgPanel), value("bg.chat", DesignTokens.bgChat), value("bg.elevated", DesignTokens.bgElevated), value("bg.input", DesignTokens.bgInput), value("text.primary", DesignTokens.textPrimary), value("text.secondary", DesignTokens.textSecondary), value("text.tertiary", DesignTokens.textTertiary), value("accent", DesignTokens.accent), value("accent.soft", DesignTokens.accentSoft), value("bubble.in", DesignTokens.bubbleIn), value("bubble.in.text", DesignTokens.bubbleInText), value("bubble.out", DesignTokens.bubbleOut), value("bubble.out.text", DesignTokens.bubbleOutText), value("success", DesignTokens.success), value("success.soft", DesignTokens.successSoft), value("warning", DesignTokens.warning), value("warning.soft", DesignTokens.warningSoft), value("danger", DesignTokens.danger), value("danger.soft", DesignTokens.dangerSoft), value("review", DesignTokens.review), value("review.soft", DesignTokens.reviewSoft), value("border.default", DesignTokens.borderDefault), value("border.subtle", DesignTokens.borderSubtle), value("code.bg", DesignTokens.codeBg)
    )
}

@Composable
fun MacBotTheme(mode: String, content: @Composable () -> Unit) {
    val systemDark = isSystemInDarkTheme()
    val translucent = mode.startsWith("translucent")
    val dark = when (mode) {
        "dark", "translucent_dark" -> true
        "light", "translucent_light" -> false
        else -> systemDark
    }
    val p = palette(dark, translucent)
    val scheme = if (dark) darkColorScheme(
        primary = p.accent, onPrimary = if (translucent) Color(0xFF111113) else DesignTokens.dark.getValue("accent.on"),
        background = p.window, surface = p.window, surfaceContainer = p.sidebar,
        surfaceContainerLow = p.panel, surfaceContainerHigh = p.elevated,
        surfaceContainerHighest = p.elevated, surfaceVariant = p.chat,
        onBackground = p.textPrimary, onSurface = p.textPrimary, onSurfaceVariant = p.textSecondary,
        secondary = p.accent, onSecondary = p.textPrimary, secondaryContainer = p.accentSoft,
        onSecondaryContainer = p.textPrimary, outline = p.borderDefault, outlineVariant = p.borderSubtle,
        error = p.danger,
    ) else lightColorScheme(
        primary = p.accent, onPrimary = Color.White, background = p.window, surface = p.window,
        surfaceContainer = p.sidebar, surfaceContainerLow = p.panel, surfaceContainerHigh = p.elevated,
        surfaceContainerHighest = p.elevated, surfaceVariant = p.chat,
        onBackground = p.textPrimary, onSurface = p.textPrimary, onSurfaceVariant = p.textSecondary,
        secondary = p.accent, onSecondary = Color.White, secondaryContainer = p.accentSoft,
        onSecondaryContainer = p.textPrimary, outline = p.borderDefault, outlineVariant = p.borderSubtle,
        error = p.danger,
    )
    CompositionLocalProvider(LocalMobilePalette provides p) {
        MaterialTheme(
            colorScheme = scheme,
            typography = androidx.compose.material3.Typography(
                displayLarge = TextStyle(fontSize = 34.sp, lineHeight = 41.sp, fontWeight = FontWeight.Bold, color = p.textPrimary),
                headlineLarge = TextStyle(fontSize = 28.sp, lineHeight = 34.sp, fontWeight = FontWeight.Bold, color = p.textPrimary),
                headlineMedium = TextStyle(fontSize = 20.sp, lineHeight = 25.sp, fontWeight = FontWeight.SemiBold, color = p.textPrimary),
                bodyLarge = TextStyle(fontSize = 16.sp, lineHeight = 22.sp, color = p.textPrimary),
                bodyMedium = TextStyle(fontSize = 15.sp, lineHeight = 21.sp, color = p.textPrimary),
                bodySmall = TextStyle(fontSize = 13.sp, lineHeight = 18.sp, color = p.textSecondary),
                titleLarge = TextStyle(fontSize = 22.sp, lineHeight = 28.sp, fontWeight = FontWeight.Medium, color = p.textPrimary),
                titleMedium = TextStyle(fontSize = 17.sp, lineHeight = 22.sp, fontWeight = FontWeight.SemiBold, color = p.textPrimary),
                titleSmall = TextStyle(fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold, color = p.textPrimary),
                labelLarge = TextStyle(fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = p.textPrimary),
                labelSmall = TextStyle(fontSize = 12.sp, lineHeight = 16.sp, color = p.textSecondary),
                labelMedium = TextStyle(fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium, color = p.textPrimary),
            ), content = content,
        )
    }
}

/** Main Bot bean glasses follow the design's 100×100 geometry. */
@Composable
fun BeanAvatar(color: Int, main: Boolean = false, status: String = "idle", avatarSize: androidx.compose.ui.unit.Dp = 48.dp) {
    val palette = LocalMobilePalette.current
    Canvas(Modifier.size(avatarSize)) {
        val radius = this.size.minDimension * .48f
        drawCircle(DesignTokens.avatarColors[color.mod(10)], radius, center)
        val sx = this.size.width / 100f
        val sy = this.size.height / 100f
        if (main) {
            val stroke = 5f * sx
            val frame = androidx.compose.ui.graphics.Path().apply {
                moveTo(24f*sx,37f*sy); lineTo(42f*sx,37f*sy); quadraticTo(49f*sx,37f*sy,49f*sx,44f*sy); lineTo(49f*sx,54f*sy); quadraticTo(49f*sx,59f*sy,44f*sx,59f*sy); lineTo(29f*sx,59f*sy); quadraticTo(24f*sx,59f*sy,24f*sx,54f*sy); close()
                moveTo(51f*sx,37f*sy); lineTo(69f*sx,37f*sy); quadraticTo(76f*sx,37f*sy,76f*sx,44f*sy); lineTo(76f*sx,54f*sy); quadraticTo(76f*sx,59f*sy,71f*sx,59f*sy); lineTo(56f*sx,59f*sy); quadraticTo(51f*sx,59f*sy,51f*sx,54f*sy); close()
            }
            drawPath(frame, DesignTokens.avatarGlasses, style = Stroke(width = stroke))
            drawLine(DesignTokens.avatarGlasses, Offset(12f*sx,41f*sy), Offset(24f*sx,44f*sy), stroke)
            drawLine(DesignTokens.avatarGlasses, Offset(76f*sx,44f*sy), Offset(88f*sx,41f*sy), stroke)
            drawLine(DesignTokens.avatarGlasses, Offset(49f*sx,45f*sy), Offset(51f*sx,45f*sy), stroke)
        } else {
            listOf(.38f, .62f).forEach { x -> drawOval(DesignTokens.avatarEye, Offset(this.size.width*x - this.size.width*.06f, this.size.height*.40f), androidx.compose.ui.geometry.Size(this.size.width*.12f, this.size.height*.18f)) }
        }
        if (status != "idle") drawCircle(
            when (status) { "waiting_user" -> palette.warning; "review" -> palette.review; "blocked" -> palette.danger; "done" -> palette.success; else -> palette.accent },
            (this.size.minDimension * .15f).coerceAtLeast(8.dp.toPx()), Offset(this.size.width*.82f,this.size.height*.82f),
        )
    }
}

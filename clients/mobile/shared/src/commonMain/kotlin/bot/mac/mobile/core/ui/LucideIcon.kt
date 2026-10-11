package bot.mac.mobile.core.ui

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/** Small, platform-neutral Lucide-style outline icons (24×24, 1.5 dp stroke). */
enum class LucideIconKind { ArrowLeft, ArrowUp, Plus, Search, RefreshCw, RotateCcw, ChevronDown, ChevronLeft, ChevronRight, Clock, CalendarDays, Ellipsis, Info, X, AtSign, Paperclip, FileText, Check, MessageCircle, Reply, Smile, Monitor, PanelRight, ShieldAlert, Target }

@Composable
fun LucideIcon(
    kind: LucideIconKind,
    modifier: Modifier = Modifier,
    tint: Color = Color.Unspecified,
    contentDescription: String? = null,
) {
    val semanticsModifier = if (contentDescription == null) modifier else modifier.semantics { this.contentDescription = contentDescription }
    val resolvedTint = if (tint == Color.Unspecified) androidx.compose.material3.MaterialTheme.colorScheme.onSurfaceVariant else tint
    Canvas(semanticsModifier) {
        val scale = size.minDimension / 24f
        if (scale <= 0f) return@Canvas
        drawContext.transform.scale(scale, scale, Offset.Zero)
        val stroke = Stroke(width = 1.5f, cap = StrokeCap.Round, join = StrokeJoin.Round)
        fun line(x1: Float, y1: Float, x2: Float, y2: Float) = drawLine(resolvedTint, Offset(x1, y1), Offset(x2, y2), 1.5f, cap = StrokeCap.Round)
        fun path(vararg points: Float) {
            val path = Path().apply {
                moveTo(points[0], points[1])
                var i = 2
                while (i + 1 < points.size) { lineTo(points[i], points[i + 1]); i += 2 }
            }
            drawPath(path, resolvedTint, style = stroke)
        }
        when (kind) {
            LucideIconKind.ArrowLeft -> { path(19f, 12f, 5f, 12f); path(12f, 19f, 5f, 12f, 12f, 5f) }
            LucideIconKind.ArrowUp -> { path(12f, 19f, 12f, 5f); path(5f, 12f, 12f, 5f, 19f, 12f) }
            LucideIconKind.Plus -> { line(12f, 5f, 12f, 19f); line(5f, 12f, 19f, 12f) }
            LucideIconKind.Search -> {
                drawCircle(resolvedTint, radius = 7f, center = Offset(10.8f, 10.8f), style = stroke)
                line(16f, 16f, 21f, 21f)
            }
            LucideIconKind.RefreshCw -> {
                path(20f, 7f, 20f, 3f, 16f, 3f)
                path(20f, 7f, 16.5f, 4f, 13.5f, 3f, 9f, 3.5f, 5.5f, 6.5f, 4f, 11f, 5f, 16f, 8.5f, 19.5f, 13f, 20f, 17f, 18f)
                path(4f, 17f, 4f, 21f, 8f, 21f)
                path(4f, 17f, 7.5f, 20f, 11f, 21f, 15f, 20.5f, 18.5f, 17.5f, 20f, 13f)
            }
            LucideIconKind.RotateCcw -> {
                path(3f, 7f, 3f, 3f, 7f, 3f)
                path(3f, 7f, 6.2f, 4.4f, 10f, 3f, 14f, 3.5f, 18f, 6f, 20f, 10f, 20f, 14f, 18f, 18f, 14f, 20f, 10f, 20f, 6f, 18f)
            }
            LucideIconKind.ChevronDown -> path(6f, 9f, 12f, 15f, 18f, 9f)
            LucideIconKind.ChevronLeft -> path(15f, 18f, 9f, 12f, 15f, 6f)
            LucideIconKind.ChevronRight -> path(9f, 18f, 15f, 12f, 9f, 6f)
            LucideIconKind.Clock -> {
                drawCircle(resolvedTint, radius = 9f, center = Offset(12f, 12f), style = stroke)
                path(12f, 7f, 12f, 12f, 15f, 14f)
            }
            LucideIconKind.CalendarDays -> {
                path(8f, 3f, 8f, 6f, 16f, 6f, 16f, 3f)
                path(4f, 8f, 20f, 8f, 20f, 19f, 4f, 19f, 4f, 5f, 20f, 5f)
                line(8f, 11f, 8f, 11.1f); line(12f, 11f, 12f, 11.1f); line(16f, 11f, 16f, 11.1f)
                line(8f, 15f, 8f, 15.1f); line(12f, 15f, 12f, 15.1f)
            }
            LucideIconKind.Ellipsis -> listOf(5f, 12f, 19f).forEach { drawCircle(resolvedTint, radius = 1f, center = Offset(it, 12f)) }
            LucideIconKind.Info -> {
                drawCircle(resolvedTint, radius = 9f, center = Offset(12f, 12f), style = stroke)
                drawCircle(resolvedTint, radius = 0.8f, center = Offset(12f, 8f))
                line(12f, 11f, 12f, 16f)
            }
            LucideIconKind.X -> { line(18f, 6f, 6f, 18f); line(6f, 6f, 18f, 18f) }
            LucideIconKind.AtSign -> {
                drawCircle(resolvedTint, radius = 9f, center = Offset(12f, 12f), style = stroke)
                drawCircle(resolvedTint, radius = 4f, center = Offset(11f, 12f), style = stroke)
                path(15f, 12f, 15f, 15f, 17f, 16f, 19f, 15f, 20f, 12f)
            }
            LucideIconKind.Paperclip -> path(8f, 12f, 14f, 6f, 17f, 9f, 10f, 16f, 7f, 13f, 14f, 6f, 17f, 6f, 20f, 9f, 20f, 12f, 11f, 21f, 8f, 21f, 4f, 17f, 4f, 14f, 13f, 5f)
            LucideIconKind.FileText -> {
                path(14f, 2f, 6f, 2f, 6f, 22f, 18f, 22f, 18f, 6f, 14f, 2f, 14f, 6f, 18f, 6f)
                line(9f, 12f, 15f, 12f); line(9f, 16f, 15f, 16f)
            }
            LucideIconKind.Check -> path(20f, 6f, 9f, 17f, 4f, 12f)
            LucideIconKind.MessageCircle -> {
                path(21f, 11.5f, 21f, 12f, 20f, 16f, 17f, 19f, 13f, 20f, 8f, 19f, 4f, 16f, 3f, 12f, 4f, 8f, 7f, 5f, 12f, 4f, 16f, 5f, 19f, 8f, 20f, 11f)
                drawCircle(resolvedTint, radius = 0.8f, center = Offset(8f, 12f)); drawCircle(resolvedTint, radius = 0.8f, center = Offset(12f, 12f)); drawCircle(resolvedTint, radius = 0.8f, center = Offset(16f, 12f))
            }
            LucideIconKind.Reply -> { path(9f, 17f, 4f, 12f, 9f, 7f); path(4f, 12f, 13f, 12f, 17f, 14f, 20f, 18f, 20f, 21f) }
            LucideIconKind.Smile -> {
                drawCircle(resolvedTint, radius = 9f, center = Offset(12f, 12f), style = stroke)
                drawCircle(resolvedTint, radius = 0.8f, center = Offset(9f, 9f)); drawCircle(resolvedTint, radius = 0.8f, center = Offset(15f, 9f))
                path(8f, 13f, 9f, 15f, 12f, 16f, 15f, 15f, 16f, 13f)
            }
            LucideIconKind.Monitor -> {
                path(3f, 4f, 21f, 4f, 21f, 17f, 3f, 17f, 3f, 4f)
                line(8f, 21f, 16f, 21f); line(12f, 17f, 12f, 21f)
            }
            LucideIconKind.PanelRight -> {
                path(3f, 4f, 21f, 4f, 21f, 20f, 3f, 20f, 3f, 4f)
                line(15f, 4f, 15f, 20f)
            }
            LucideIconKind.ShieldAlert -> {
                path(12f, 3f, 20f, 6f, 19f, 13f, 16f, 18f, 12f, 21f, 8f, 18f, 5f, 13f, 4f, 6f, 12f, 3f)
                line(12f, 8f, 12f, 13f); drawCircle(resolvedTint, radius = 0.8f, center = Offset(12f, 16f))
            }
            LucideIconKind.Target -> {
                drawCircle(resolvedTint, radius = 8f, center = Offset(12f, 12f), style = stroke)
                drawCircle(resolvedTint, radius = 3f, center = Offset(12f, 12f), style = stroke)
                line(12f, 2f, 12f, 4f); line(12f, 20f, 12f, 22f); line(2f, 12f, 4f, 12f); line(20f, 12f, 22f, 12f)
            }
        }
        drawContext.transform.scale(1f / scale, 1f / scale, Offset.Zero)
    }
}

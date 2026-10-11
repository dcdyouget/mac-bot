package bot.mac.mobile.feature.chat

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.DesignMotion
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.design.LocalSystemAnimationsEnabled
import bot.mac.mobile.resources.*
import org.jetbrains.compose.resources.stringResource

/** Localizes protocol status values before they reach the UI. */
@Composable
fun StatusLabel(status: String, modifier: Modifier = Modifier) {
    val resource = when (status.lowercase()) {
        "none", "idle" -> Res.string.status_idle
        "working", "active", "running", "continued" -> Res.string.status_working
        "waiting_user" -> Res.string.status_waiting_user
        "waiting_bot" -> Res.string.status_standby
        "queued" -> Res.string.status_queued
        "done", "completed", "confirmed", "approved" -> Res.string.status_done
        "stopped", "cancelled", "canceled", "paused", "ended", "skipped" -> Res.string.status_cancelled
        "blocked" -> Res.string.status_blocked
        "failed", "denied" -> Res.string.status_failed
        "review", "pending" -> Res.string.status_review
        "changes_requested" -> Res.string.feature_status_changes_requested
        "expired" -> Res.string.feature_decision_expired
        "archived" -> Res.string.project_status_archived
        "unread" -> Res.string.feature_status_unread
        else -> Res.string.feature_status_unknown
    }
    val label = stringResource(resource)
    val announcesChanges = status.lowercase() in setOf(
        "working", "active", "running", "continued", "waiting_user", "waiting_bot", "queued",
        "done", "completed", "confirmed", "approved", "blocked", "failed", "denied", "review", "pending",
        "stopped", "cancelled", "canceled", "paused", "ended", "skipped", "changes_requested", "expired", "archived",
    )
    val palette = LocalMobilePalette.current
    val (glyph, foreground, background) = when (status.lowercase()) {
        "none", "idle" -> Triple("○", palette.textTertiary, palette.tokenColors["bg.active"] ?: palette.borderSubtle)
        "working", "active", "running", "continued" -> Triple("⟳", palette.accent, palette.accentSoft)
        "waiting_user", "waiting_bot" -> Triple("⚠", palette.warning, palette.warningSoft)
        "done", "completed", "confirmed", "approved" -> Triple("✓", palette.success, palette.successSoft)
        "stopped", "cancelled", "canceled", "paused", "ended", "skipped", "expired" -> Triple("○", palette.textSecondary, palette.tokenColors["bg.active"] ?: palette.borderSubtle)
        "blocked", "failed", "denied" -> Triple("!", palette.danger, palette.dangerSoft)
        "review", "pending" -> Triple("◎", palette.review, palette.reviewSoft)
        "queued" -> Triple("…", palette.textSecondary, palette.tokenColors["bg.active"] ?: palette.borderSubtle)
        else -> Triple("•", palette.textSecondary, palette.tokenColors["bg.active"] ?: palette.borderSubtle)
    }
    Row(
        modifier.clip(RoundedCornerShape(999.dp)).background(background).padding(horizontal = 6.dp, vertical = 2.dp).semantics(mergeDescendants = true) {
            stateDescription = label
            if (announcesChanges) liveRegion = LiveRegionMode.Polite
        },
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (status.lowercase() in setOf("working", "active", "running", "continued")) WorkingGlyph(glyph, foreground)
        else Text(glyph, color = foreground, style = MaterialTheme.typography.labelSmall, modifier = Modifier.clearAndSetSemantics {})
        Text(label, color = foreground, style = MaterialTheme.typography.labelMedium)
    }
}

@Composable
private fun WorkingGlyph(glyph: String, color: androidx.compose.ui.graphics.Color) {
    if (LocalSystemAnimationsEnabled.current) {
        val transition = rememberInfiniteTransition(label = "status-spinner")
        val rotation = transition.animateFloat(
            initialValue = 0f,
            targetValue = 360f,
            animationSpec = infiniteRepeatable(tween(DesignMotion.workingRotationMillis, easing = LinearEasing), RepeatMode.Restart),
            label = "status-spinner-rotation",
        ).value
        Text(glyph, color = color, style = MaterialTheme.typography.labelSmall, modifier = Modifier.graphicsLayer(rotationZ = rotation).clearAndSetSemantics {})
    } else Text(glyph, color = color, style = MaterialTheme.typography.labelSmall, modifier = Modifier.clearAndSetSemantics {})
}

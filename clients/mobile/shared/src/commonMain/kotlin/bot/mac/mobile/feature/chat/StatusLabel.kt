package bot.mac.mobile.feature.chat

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import bot.mac.mobile.resources.*
import org.jetbrains.compose.resources.stringResource

/** Localizes protocol status values before they reach the UI. */
@Composable
fun StatusLabel(status: String, modifier: Modifier = Modifier) {
    val resource = when (status.lowercase()) {
        "none", "idle" -> Res.string.feature_status_idle
        "working", "active", "running", "continued" -> Res.string.feature_status_working
        "waiting_user" -> Res.string.feature_status_waiting_user
        "waiting_bot" -> Res.string.feature_status_waiting_bot
        "queued" -> Res.string.feature_status_queued
        "done", "completed", "confirmed", "approved" -> Res.string.feature_status_done
        "stopped", "cancelled", "canceled", "paused", "ended", "skipped" -> Res.string.feature_status_stopped
        "blocked", "failed", "denied" -> Res.string.feature_status_blocked
        "review", "pending" -> Res.string.feature_status_review
        "changes_requested" -> Res.string.feature_status_changes_requested
        "archived" -> Res.string.feature_status_archived
        "unread" -> Res.string.feature_status_unread
        else -> Res.string.feature_status_unknown
    }
    Text(stringResource(resource), modifier = modifier, style = MaterialTheme.typography.labelMedium)
}

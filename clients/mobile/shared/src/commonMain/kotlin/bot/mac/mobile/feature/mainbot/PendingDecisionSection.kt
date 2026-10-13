package bot.mac.mobile.feature.mainbot

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileState
import bot.mac.mobile.core.ui.LucideIcon
import bot.mac.mobile.core.ui.LucideIconKind
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.feature_bot
import bot.mac.mobile.resources.followup_pending_approval
import bot.mac.mobile.resources.followup_pending_decisions
import bot.mac.mobile.resources.followup_pending_decisions_empty
import bot.mac.mobile.resources.followup_pending_question
import bot.mac.mobile.resources.followup_pending_review
import org.jetbrains.compose.resources.stringResource

@Composable
internal fun PendingDecisionSection(
    decisions: List<PendingDecision>,
    state: MobileState,
    onClick: (PendingDecision) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(Res.string.followup_pending_decisions), Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            Text(decisions.size.toString(), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (decisions.isEmpty()) {
            Text(stringResource(Res.string.followup_pending_decisions_empty), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        } else decisions.forEach { decision ->
            val botName = state.bots.firstOrNull { it.str("id") == decision.botId }?.str("name")
                ?.takeIf(String::isNotBlank) ?: stringResource(Res.string.feature_bot)
            val projectName = state.projects.firstOrNull { it.str("id") == decision.projectId }?.str("name")
                ?.takeIf(String::isNotBlank)
            val chat = state.chats.firstOrNull { it.str("id") == decision.chatId }
            val chatName = chat?.str("title")?.takeIf(String::isNotBlank)
                ?: chat?.str("bot_id")?.let { id -> state.bots.firstOrNull { it.str("id") == id }?.str("name") }
                ?: botName
            val context = projectName ?: chatName
            val kindLabel = when (decision.kind) {
                PendingDecisionKind.APPROVAL -> stringResource(Res.string.followup_pending_approval)
                PendingDecisionKind.QUESTION -> stringResource(Res.string.followup_pending_question)
                PendingDecisionKind.REVIEW -> stringResource(Res.string.followup_pending_review)
            }
            val title = when (decision.kind) {
                PendingDecisionKind.APPROVAL -> listOf(decision.tool, decision.title).filter(String::isNotBlank).distinct().joinToString(" · ")
                    .ifBlank { kindLabel }
                else -> decision.title.ifBlank { kindLabel }
            }
            val icon = when (decision.kind) {
                PendingDecisionKind.APPROVAL -> LucideIconKind.ShieldAlert
                PendingDecisionKind.QUESTION -> LucideIconKind.MessageCircle
                PendingDecisionKind.REVIEW -> LucideIconKind.Check
            }
            val detail = listOf(botName, context, decision.risk.takeIf(String::isNotBlank) ?: kindLabel)
                .distinct().filter(String::isNotBlank).joinToString(" · ")
            val canNavigate = decision.chatId.isNotBlank() || decision.kind != PendingDecisionKind.REVIEW
            Surface(
                modifier = Modifier.fillMaxWidth().clickable(enabled = canNavigate) { onClick(decision) },
                shape = RoundedCornerShape(12.dp),
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
            ) {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    LucideIcon(icon, Modifier.size(18.dp), tint = if (decision.kind == PendingDecisionKind.APPROVAL) LocalMobilePalette.current.warning else MaterialTheme.colorScheme.primary)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(title, style = MaterialTheme.typography.bodyMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                    LucideIcon(LucideIconKind.ChevronRight, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

package bot.mac.mobile.feature.chat

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.design.LocalMobilePalette
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.chat_empty_bot_dm
import bot.mac.mobile.resources.chat_empty_direct
import bot.mac.mobile.resources.chat_empty_group
import bot.mac.mobile.resources.chat_empty_group_title
import bot.mac.mobile.resources.chat_empty_title
import org.jetbrains.compose.resources.stringResource

@Composable
internal fun ConversationEmptyState(kind: String) {
    val palette = LocalMobilePalette.current
    val title = if (kind == "project") {
        stringResource(Res.string.chat_empty_group_title)
    } else {
        stringResource(Res.string.chat_empty_title)
    }
    val detail = when (kind) {
        "project" -> stringResource(Res.string.chat_empty_group)
        "bot_dm" -> stringResource(Res.string.chat_empty_bot_dm)
        else -> stringResource(Res.string.chat_empty_direct)
    }
    Surface(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 20.dp),
        color = palette.sidebar,
        shape = RoundedCornerShape(16.dp),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(title, style = MaterialTheme.typography.titleSmall)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = palette.textSecondary)
        }
    }
}

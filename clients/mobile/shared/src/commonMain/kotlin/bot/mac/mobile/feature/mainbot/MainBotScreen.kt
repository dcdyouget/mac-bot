package bot.mac.mobile.feature.mainbot

import androidx.compose.runtime.Composable
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.feature.chat.ChatScreen

/** Main Bot uses the same private-chat interaction model, with project cards routed to groups. */
@Composable
fun MainBotScreen(
    repository: MobileRepository,
    chatId: String,
    onOpenGroup: (String) -> Unit = {},
    onBack: () -> Unit = {},
) {
    ChatScreen(
        repository = repository,
        chatId = chatId,
        onOpenProject = onOpenGroup,
        onBack = onBack,
    )
}


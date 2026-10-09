package bot.mac.mobile.feature.mainbot

import androidx.compose.runtime.Composable
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.feature.chat.ChatScreen

/** Main Bot uses the private-chat interaction model and routes every actionable block. */
@Composable
fun MainBotScreen(
    repository: MobileRepository,
    chatId: String,
    onOpenTrace: (assignmentId: String?, chatId: String?) -> Unit = { _, _ -> },
    onOpenProject: (String) -> Unit = {},
    onOpenArtifact: (artifactId: String, pathOrUrl: String, projectId: String?) -> Unit = { _, _, _ -> },
    onOpenHistory: (String) -> Unit = {},
    onBack: () -> Unit = {},
    onProjectAction: (projectId: String, action: String) -> Unit = { _, _ -> },
    onOpenScreen: (botId: String, tabId: String?) -> Unit = { _, _ -> },
    onOpenChat: (String) -> Unit = {},
    onLoopAction: (rootMessageId: String, action: String) -> Unit = { _, _ -> },
    onTakeover: (botId: String) -> Unit = {},
) {
    ChatScreen(
        repository = repository,
        chatId = chatId,
        onOpenTrace = onOpenTrace,
        onOpenProject = onOpenProject,
        onOpenArtifact = onOpenArtifact,
        onOpenHistory = onOpenHistory,
        onBack = onBack,
        onProjectAction = onProjectAction,
        onOpenScreen = onOpenScreen,
        onOpenChat = onOpenChat,
        onLoopAction = onLoopAction,
        onTakeover = onTakeover,
    )
}

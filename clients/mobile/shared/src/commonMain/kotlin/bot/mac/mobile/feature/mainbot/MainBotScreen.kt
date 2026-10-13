package bot.mac.mobile.feature.mainbot

import androidx.compose.runtime.Composable
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.feature.chat.ChatScreen

/** Main Bot uses the private-chat interaction model and routes every actionable block. */
@Composable
fun MainBotScreen(
    repository: MobileRepository,
    chatId: String,
    initialMessageId: String? = null,
    initialScrollAnchorMessageId: String? = null,
    initialScrollAnchorOffsetPx: Int = 0,
    onRememberTraceAnchor: (chatId: String, messageId: String, offsetPx: Int) -> Unit = { _, _, _ -> },
    onTraceAnchorRestored: (chatId: String) -> Unit = {},
    onInitialMessageConsumed: (chatId: String, messageId: String) -> Unit = { _, _ -> },
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
    onOpenFollowup: (chatId: String) -> Unit = {},
    onCreateSkill: (String) -> Unit = {},
) {
    ChatScreen(
        repository = repository,
        chatId = chatId,
        initialMessageId = initialMessageId,
        initialScrollAnchorMessageId = initialScrollAnchorMessageId,
        initialScrollAnchorOffsetPx = initialScrollAnchorOffsetPx,
        onRememberTraceAnchor = onRememberTraceAnchor,
        onTraceAnchorRestored = onTraceAnchorRestored,
        onInitialMessageConsumed = onInitialMessageConsumed,
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
        onOpenFollowup = onOpenFollowup,
        onCreateSkill = onCreateSkill,
    )
}

package bot.mac.mobile.core.ui

internal fun mainBotChatTitle(chatTitle: String?, serverBotName: String?, fallbackName: String): String =
    chatTitle?.takeIf(String::isNotBlank)
        ?: serverBotName?.takeIf(String::isNotBlank)
        ?: fallbackName

internal fun botDisplayName(name: String?, botId: String, isMain: Boolean, mainDefaultName: String): String =
    name?.takeIf(String::isNotBlank)
        ?: if (isMain) mainDefaultName else botId

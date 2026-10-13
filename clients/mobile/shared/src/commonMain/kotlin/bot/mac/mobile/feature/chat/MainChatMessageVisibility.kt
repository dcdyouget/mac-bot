package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

/** Mirrors the desktop timeline rule: workers stay visible in their own chats, not in the main DM. */
internal fun visibleMessagesForChat(
    messages: List<JsonObject>,
    chat: JsonObject?,
    bots: List<JsonObject>,
): List<JsonObject> {
    val chatBotId = chat?.str("bot_id").orEmpty()
    val mainBotId = if (
        chat?.str("kind") == "main" ||
        (chat?.str("project_id").isNullOrBlank() && bots.any { it.str("id") == chatBotId && it.boolean("is_main") })
    ) bots.firstOrNull { it.boolean("is_main") }?.str("id")?.takeIf(String::isNotBlank)
        ?: chatBotId.takeIf(String::isNotBlank)
        ?: "bot_main" else null

    return messages.filter { message ->
        val messageChatId = message.str("chat_id")
        if (!messageChatId.isBlank() && messageChatId != chat?.str("id")) return@filter false
        val sender = message.obj("sender")
        mainBotId == null || sender.str("kind") != "bot" || sender.str("bot_id") == mainBotId
    }
}

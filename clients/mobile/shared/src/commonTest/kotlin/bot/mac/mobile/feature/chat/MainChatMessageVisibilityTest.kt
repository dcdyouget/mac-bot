package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.protocol.str
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

class MainChatMessageVisibilityTest {
    private val mainChat = buildJsonObject {
        put("id", "chat_main")
        put("kind", "main")
        put("bot_id", "main-1")
    }
    private val bots = listOf(
        buildJsonObject { put("id", "main-1"); put("is_main", true) },
        buildJsonObject { put("id", "worker-1"); put("is_main", false) },
    )

    private fun message(id: String, senderKind: String, botId: String? = null, chatId: String = "chat_main") =
        buildJsonObject {
            put("id", id)
            put("chat_id", chatId)
            putJsonObject("sender") {
                put("kind", senderKind)
                botId?.let { put("bot_id", it) }
            }
        }

    @Test
    fun mainChatHidesWorkerDirectMessagesButKeepsSystemTaskRows() {
        val messages = listOf(
            message("user", "user"),
            message("secretary", "bot", "main-1"),
            message("worker", "bot", "worker-1"),
            message("task", "system"),
        )

        assertEquals(listOf("user", "secretary", "task"), visibleMessagesForChat(messages, mainChat, bots).map { it.str("id") })
    }

    @Test
    fun botAndGroupChatsKeepBotMessages() {
        val messages = listOf(message("worker", "bot", "worker-1", "dm-worker"))

        assertEquals(messages, visibleMessagesForChat(messages, buildJsonObject {
            put("id", "dm-worker")
            put("kind", "bot_dm")
            put("bot_id", "worker-1")
        }, bots))
    }

    @Test
    fun mainChatUsesItsBotIdBeforeTheBotListHasLoaded() {
        val messages = listOf(
            message("main", "bot", "main-1"),
            message("worker", "bot", "worker-1"),
        )

        assertEquals(listOf("main"), visibleMessagesForChat(messages, mainChat, emptyList()).map { it.str("id") })
    }
}

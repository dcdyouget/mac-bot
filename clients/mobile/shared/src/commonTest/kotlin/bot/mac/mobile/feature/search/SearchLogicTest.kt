package bot.mac.mobile.feature.search

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class SearchLogicTest {
    @Test
    fun onlyProtocolSearchKindsAreForwarded() {
        assertEquals("message", searchKindParameter("message"))
        assertEquals("routine", searchKindParameter("routine"))
        assertNull(searchKindParameter("unknown"))
    }

    @Test
    fun messageResultOpensItsChatAtTheMatchedMessage() {
        val result = buildJsonObject {
            put("kind", "message")
            put("id", "old-message")
            put("chat_id", "group-chat")
        }

        assertEquals(SearchChatTarget("group-chat", "old-message"), searchChatTarget(result))
    }

    @Test
    fun chatResultUsesItsOwnIdWithoutAMessageJump() {
        assertEquals(SearchChatTarget("group-chat"), searchChatTarget(buildJsonObject {
            put("kind", "chat")
            put("id", "group-chat")
        }))
        assertNull(searchChatTarget(buildJsonObject { put("kind", "message"); put("id", "message-without-chat") }))
        assertNull(searchChatTarget(buildJsonObject { put("kind", "unknown"); put("chat_id", "group-chat") }))
    }
}

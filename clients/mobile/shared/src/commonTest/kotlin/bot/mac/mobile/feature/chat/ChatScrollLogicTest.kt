package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

class ChatScrollLogicTest {
    @Test
    fun viewportCannotDisableFollowLatestBeforeInitialPosition() {
        val state = ChatScrollState().onViewportChanged(atBottom = false)

        assertFalse(state.initialPositioned)
        assertTrue(state.followLatest)
    }

    @Test
    fun initialPositionEnablesFollowLatestAndUserScrollCanDisableIt() {
        val positioned = ChatScrollState().afterInitialPosition()

        assertTrue(positioned.shouldScrollToLatest(messageCount = 3))
        assertFalse(positioned.onViewportChanged(atBottom = false).followLatest)
        assertTrue(positioned.onViewportChanged(atBottom = true).followLatest)
    }

    @Test
    fun latestIndexAccountsForOlderMessagesSentinel() {
        assertEquals(3, latestChatItemIndex(messageCount = 4, historyHasMore = false))
        assertEquals(4, latestChatItemIndex(messageCount = 4, historyHasMore = true))
        assertNull(latestChatItemIndex(messageCount = 0, historyHasMore = true))
    }

    @Test
    fun reactedMessageEndsVisualGroupSoItsTapbacksRemainVisible() {
        fun message(withReaction: Boolean) = buildJsonObject {
            put("created_at", "2026-10-12T10:00:00Z")
            putJsonObject("sender") { put("kind", "user") }
            if (withReaction) put("reactions", buildJsonArray { add(buildJsonObject {}) })
        }

        assertFalse(continuesChatMessageGroup(message(withReaction = true), message(withReaction = false)))
        assertTrue(continuesChatMessageGroup(message(withReaction = false), message(withReaction = false)))
    }
}

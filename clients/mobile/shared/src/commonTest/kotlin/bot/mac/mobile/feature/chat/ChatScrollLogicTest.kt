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
        val messages = (0 until 4).map { buildJsonObject { put("created_at", "2026-10-12T10:00:00Z") } }
        assertEquals(4, latestChatItemIndex(messages, historyHasMore = false))
        assertEquals(5, latestChatItemIndex(messages, historyHasMore = true))
        assertNull(latestChatItemIndex(emptyList(), historyHasMore = true))
    }

    @Test
    fun messageIndexIncludesStickyDayHeaderAndTimeSeparators() {
        val messages = listOf(
            buildJsonObject { put("created_at", "2026-10-12T10:00:00Z") },
            buildJsonObject { put("created_at", "2026-10-12T10:07:00Z") },
            buildJsonObject { put("created_at", "2026-10-13T10:00:00Z") },
        )

        assertEquals(1, chatMessageLazyIndex(messages, 0, historyHasMore = false))
        assertEquals(3, chatMessageLazyIndex(messages, 1, historyHasMore = false))
        assertEquals(5, chatMessageLazyIndex(messages, 2, historyHasMore = false))
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

    @Test
    fun deepLinkIndexUsesExpandedMessageHistory() {
        val expandedHistory = (0..100).map { index ->
            buildJsonObject {
                put("id", "message-$index")
                put("created_at", "2026-10-12T10:00:00Z")
            }
        }

        assertEquals(102, chatMessageLazyIndex(expandedHistory, 100, historyHasMore = true))
    }
}

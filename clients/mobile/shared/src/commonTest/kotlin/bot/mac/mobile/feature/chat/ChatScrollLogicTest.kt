package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

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
}

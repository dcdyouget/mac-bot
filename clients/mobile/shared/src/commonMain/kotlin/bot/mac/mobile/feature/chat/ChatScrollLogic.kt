package bot.mac.mobile.feature.chat

import kotlinx.serialization.json.JsonObject

internal data class ChatScrollState(
    val initialPositioned: Boolean = false,
    val followLatest: Boolean = true,
)

internal fun ChatScrollState.afterInitialPosition(): ChatScrollState =
    copy(initialPositioned = true, followLatest = true)

internal fun ChatScrollState.onViewportChanged(atBottom: Boolean): ChatScrollState =
    if (!initialPositioned) this else copy(followLatest = atBottom)

internal fun ChatScrollState.shouldScrollToLatest(messageCount: Int): Boolean =
    initialPositioned && followLatest && messageCount > 0

internal fun chatMessageLazyIndex(messages: List<JsonObject>, targetMessageIndex: Int, historyHasMore: Boolean): Int {
    require(targetMessageIndex in messages.indices)
    var index = if (historyHasMore) 1 else 0
    for (messageIndex in 0..targetMessageIndex) {
        val message = messages[messageIndex]
        val previous = messages.getOrNull(messageIndex - 1)
        if (previous == null || !sameChatDay(previous, message)) index++
        if (previous != null && hasChatTimeSeparator(previous, message)) index++
        index++
    }
    return index - 1
}

internal fun latestChatItemIndex(messages: List<JsonObject>, historyHasMore: Boolean): Int? =
    messages.indices.lastOrNull()?.let { chatMessageLazyIndex(messages, it, historyHasMore) }

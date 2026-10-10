package bot.mac.mobile.feature.chat

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

internal fun latestChatItemIndex(messageCount: Int, historyHasMore: Boolean): Int? =
    messageCount.takeIf { it > 0 }?.let { it - 1 + if (historyHasMore) 1 else 0 }

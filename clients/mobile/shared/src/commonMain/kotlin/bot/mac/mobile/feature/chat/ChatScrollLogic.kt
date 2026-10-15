package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

internal data class ChatScrollState(
    val initialPositioned: Boolean = false,
    val followLatest: Boolean = true,
)

internal data class ChatMessageScrollAnchor(val messageId: String, val offsetPx: Int)

internal enum class InitialChatPositionKind { MESSAGE_LINK, TRACE_RETURN }

internal data class InitialChatPosition(val messageId: String, val kind: InitialChatPositionKind)

/** A date can occur in more than one sequence segment when timestamps arrive out of order. */
internal fun chatDayHeaderKey(hostId: String, chatId: String, dayKey: String, firstMessage: JsonObject): String {
    val segment = firstMessage.str("id").ifBlank {
        firstMessage["seq"]?.toString()?.trim('"')?.takeIf { it.isNotBlank() }
            ?: "message:${firstMessage.hashCode()}"
    }
    return "$hostId:$chatId:day:$dayKey:$segment"
}

/** A saved trace-return position wins over an older route/deep-link target. */
internal fun resolveInitialChatPosition(
    initialMessageId: String?,
    traceAnchorMessageId: String?,
): InitialChatPosition? = when {
    !traceAnchorMessageId.isNullOrBlank() -> InitialChatPosition(traceAnchorMessageId, InitialChatPositionKind.TRACE_RETURN)
    !initialMessageId.isNullOrBlank() -> InitialChatPosition(initialMessageId, InitialChatPositionKind.MESSAGE_LINK)
    else -> null
}

internal fun captureChatMessageAnchor(messageId: String, viewportStartOffset: Int, itemOffset: Int): ChatMessageScrollAnchor =
    ChatMessageScrollAnchor(messageId, viewportStartOffset - itemOffset)

internal fun ChatScrollState.afterInitialPosition(followLatest: Boolean = true): ChatScrollState =
    copy(initialPositioned = true, followLatest = followLatest)

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

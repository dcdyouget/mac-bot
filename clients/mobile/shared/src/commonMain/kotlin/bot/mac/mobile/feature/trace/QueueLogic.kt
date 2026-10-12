package bot.mac.mobile.feature.trace

import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

internal data class QueuedSteer(
    val message: JsonObject,
    val delivery: JsonObject,
    val key: String,
)

internal fun assignmentChatFor(assignment: JsonObject?, chats: List<JsonObject>): JsonObject? {
    if (assignment == null) return null
    val originChatId = assignment.str("origin_chat_id")
    if ("origin_chat_id" in assignment) {
        return chats.firstOrNull { originChatId.isNotBlank() && it.str("id") == originChatId }
    }

    val projectId = assignment.str("project_id")
    return chats.firstOrNull { projectId.isNotBlank() && it.str("project_id") == projectId }
}

/** Selects Bot-queued deliveries for an assignment, independently of human chat read state. */
internal fun queuedSteerMessages(messages: List<JsonObject>, assignmentId: String, botId: String): List<QueuedSteer> {
    if (assignmentId.isBlank() || botId.isBlank()) return emptyList()

    return messages.flatMap { message ->
        message.arr("delivery").mapNotNull { it as? JsonObject }
            .filter { delivery ->
                delivery.str("assignment_id") == assignmentId &&
                    delivery.str("bot_id") == botId &&
                    delivery.str("state") == "queued"
            }
            .map { delivery ->
                val key = "${message.str("id")}:${delivery.str("bot_id")}:$assignmentId"
                QueuedSteer(message, delivery, key)
            }
    }
        .distinctBy { it.key }
        .sortedBy { it.message.long("seq") }
}

internal fun assignmentSteeredMessageIds(assignment: JsonObject?): Set<String> =
    assignment?.arr("steers")
        ?.mapNotNull { (it as? JsonObject)?.str("message_id")?.takeIf(String::isNotBlank) }
        ?.toSet()
        .orEmpty()

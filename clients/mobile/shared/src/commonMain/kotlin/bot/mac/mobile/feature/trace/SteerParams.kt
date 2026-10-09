package bot.mac.mobile.feature.trace

import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Builds the protocol payload for a user steer sent to an assignment's origin chat. */
internal fun buildSteerChatParams(assignment: JsonObject, text: String): JsonObject = buildJsonObject {
    put("chat_id", assignment.str("origin_chat_id"))
    put("text", text)
    put("mentions", buildJsonArray {
        add(buildJsonObject {
            put("kind", "bot")
            put("bot_id", assignment.str("bot_id"))
            put("instruction", JsonNull)
        })
    })
}

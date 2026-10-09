package bot.mac.mobile.feature.trace

import kotlin.test.Test
import kotlin.test.assertEquals
import bot.mac.mobile.core.protocol.arr
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put

class SteerParamsTest {
    @Test
    fun usesAssignmentOriginChatAndBotMention() {
        val assignment = buildJsonObject {
            put("id", "asg_1")
            put("bot_id", "bot_1")
            put("origin_chat_id", "chat_origin")
        }

        val params = buildSteerChatParams(assignment, "先只做邮箱登录")

        assertEquals("chat_origin", params["chat_id"]?.toString()?.trim('"'))
        assertEquals("先只做邮箱登录", params["text"]?.toString()?.trim('"'))
        assertEquals("bot_1", params.arr("mentions").first().jsonObject["bot_id"]?.toString()?.trim('"'))
    }
}

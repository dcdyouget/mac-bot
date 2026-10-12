package bot.mac.mobile.feature.trace

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject

class QueueLogicTest {
    @Test
    fun assignmentOriginChatWinsWhenProjectHasSeveralChats() {
        val assignment = """{"origin_chat_id":"chat_login","project_id":"prj_login"}"""
        val chats = listOf(
            """{"id":"chat_project_other","project_id":"prj_login"}""",
            """{"id":"chat_login","project_id":"prj_login"}""",
        ).map { Json.parseToJsonElement(it).jsonObject }

        assertEquals("chat_login", assignmentChatFor(Json.parseToJsonElement(assignment).jsonObject, chats)?.str("id"))
        val legacy = Json.parseToJsonElement("""{"project_id":"prj_login"}""").jsonObject
        assertEquals("chat_project_other", assignmentChatFor(legacy, chats)?.str("id"))
        val originNotLoaded = Json.parseToJsonElement("""{"origin_chat_id":"chat_not_loaded","project_id":"prj_login"}""").jsonObject
        assertNull(assignmentChatFor(originNotLoaded, chats))
        assertNull(assignmentChatFor(null, chats))
    }

    @Test
    fun selectsQueuedTargetDeliveriesWithoutTopLevelAssignmentOrHumanReadCursor() {
        val messages = listOf(
            """{"id":"msg_queued","seq":18,"assignment_id":null,"delivery":[{"assignment_id":"asg_1","bot_id":"bot_code","state":"queued"}]}""",
            """{"id":"msg_other_assignment","seq":19,"delivery":[{"assignment_id":"asg_2","bot_id":"bot_code","state":"queued"}]}""",
            """{"id":"msg_delivered","seq":20,"delivery":[{"assignment_id":"asg_1","bot_id":"bot_code","state":"delivered"}]}""",
            """{"id":"msg_wrong_bot","seq":21,"delivery":[{"assignment_id":"asg_1","bot_id":"bot_test","state":"queued"}]}""",
        ).map { Json.parseToJsonElement(it).jsonObject }

        val selected = queuedSteerMessages(messages, "asg_1", "bot_code")

        assertEquals(listOf("msg_queued"), selected.map { it.message.str("id") })
        assertEquals("bot_code", selected.single().delivery.str("bot_id"))
        assertEquals("msg_queued:bot_code:asg_1", selected.single().key)
        assertEquals(emptyList(), queuedSteerMessages(messages, "", "bot_code"))
    }

    @Test
    fun assignmentSteerSnapshotIdentifiesPromotedMessage() {
        val assignment = Json.parseToJsonElement(
            """{"steers":[{"message_id":"msg_1","text":"continue","applied_at":null},{"message_id":""}]}""",
        ).jsonObject

        assertEquals(setOf("msg_1"), assignmentSteeredMessageIds(assignment))
        assertEquals(emptySet(), assignmentSteeredMessageIds(Json.parseToJsonElement("{} ").jsonObject))
        assertEquals(emptySet(), assignmentSteeredMessageIds(null))
    }
}

package bot.mac.mobile.core.state

import bot.mac.mobile.core.protocol.protocolJson
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class RpcStateReducerTest {
    private fun objectOf(value: String): JsonObject =
        protocolJson.parseToJsonElement(value).jsonObject

    @Test
    fun workbenchGetKeepsBotListAndAbsorbsAssignments() {
        val current = MobileState(
            bots = listOf(objectOf("""{"id":"existing","name":"Existing"}""")),
            assignments = listOf(objectOf("""{"id":"old-assignment"}""")),
        )
        val result = objectOf(
            """{
                "bots":[{"id":"wrapper","assignments":[{"id":"active-assignment"}]}],
                "done_today":[{"id":"done-assignment"}],
                "summary":{"total":2}
            }"""
        )

        val next = RpcStateReducer.apply(current, "workbench.get", buildJsonObject {}, result)

        assertEquals(listOf("existing"), next.bots.map { it.str("id") })
        assertEquals(
            setOf("old-assignment", "active-assignment", "done-assignment"),
            next.assignments.map { it.str("id") }.toSet(),
        )
        assertEquals(result, next.workbench)
    }

    @Test
    fun botListReplacesTheCachedBotCollection() {
        val current = MobileState(
            bots = listOf(objectOf("""{"id":"old"}""")),
        )
        val result = objectOf("""{"bots":[{"id":"new-1"},{"id":"new-2"}]}""")

        val next = RpcStateReducer.apply(current, "bot.list", buildJsonObject {}, result)

        assertEquals(listOf("new-1", "new-2"), next.bots.map { it.str("id") })
    }

    @Test
    fun workbenchWaitingRefreshMergesPendingApprovalsAndQuestions() {
        val current = MobileState(
            approvals = listOf(objectOf("""{"id":"approval-existing","state":"pending"}""")),
            questions = listOf(objectOf("""{"id":"question-existing","state":"pending"}""")),
        )
        val result = objectOf(
            """{
                "waiting":[
                    {"kind":"approval","approval":{"id":"approval-new","state":"pending"}},
                    {"kind":"question","question":{"id":"question-new","state":"pending"}},
                    {"kind":"takeover","bot_id":"bot-1","assignment_id":"assignment-1","reason":"user"}
                ],
                "bots":[],
                "done_today":[]
            }"""
        )

        val next = RpcStateReducer.apply(current, "workbench.get", buildJsonObject {}, result)

        assertEquals(
            setOf("approval-existing", "approval-new"),
            next.approvals.map { it.str("id") }.toSet(),
        )
        assertEquals(
            setOf("question-existing", "question-new"),
            next.questions.map { it.str("id") }.toSet(),
        )
        assertTrue(next.bots.isEmpty())
    }

    @Test
    fun templateCreateMergesBotsAndHistoryKeepsExistingMessages() {
        val current = MobileState(
            bots = listOf(objectOf("""{"id":"existing-bot"}""")),
            chats = listOf(objectOf("""{"id":"chat-1","kind":"direct"}""")),
            messages = mapOf(
                "chat-1" to listOf(objectOf("""{"id":"m1","chat_id":"chat-1","seq":1,"fallback_text":"first"}""")),
            ),
        )
        val created = objectOf(
            """{
                "bots":[{"id":"created-bot"}],
                "dm_chats":[{"id":"chat-2","kind":"direct"}]
            }"""
        )
        val afterCreate = RpcStateReducer.apply(
            current,
            "bot.create_from_template",
            buildJsonObject { put("template_id", "team") },
            created,
        )
        val afterHistory = RpcStateReducer.apply(
            afterCreate,
            "chat.history",
            buildJsonObject { put("chat_id", "chat-1") },
            objectOf("""{"messages":[{"id":"m2","chat_id":"chat-1","seq":2,"fallback_text":"second"}]}"""),
        )

        assertEquals(setOf("existing-bot", "created-bot"), afterCreate.bots.map { it.str("id") }.toSet())
        assertTrue(afterCreate.chats.any { it.str("id") == "chat-1" })
        assertTrue(afterCreate.chats.any { it.str("id") == "chat-2" })
        assertEquals(listOf("m1", "m2"), afterHistory.messages.getValue("chat-1").map { it.str("id") })
    }

    @Test
    fun skillImportAndProviderModelCrudMergeByStableKeys() {
        val current = MobileState(
            skills = listOf(objectOf("""{"name":"existing-skill"}""")),
            providers = listOf(objectOf("""{"id":"old-provider"}""")),
            models = listOf(objectOf("""{"ref":"old-provider/old-model"}""")),
        )
        val imported = RpcStateReducer.apply(
            current,
            "skill.import",
            buildJsonObject {},
            objectOf("""{"skills":[{"name":"imported-skill"}]}"""),
        )
        val provider = RpcStateReducer.apply(
            imported,
            "provider.create",
            buildJsonObject {},
            objectOf("""{"provider":{"id":"new-provider"},"models":[{"ref":"new-provider/model-1"}]}"""),
        )
        val upserted = RpcStateReducer.apply(
            provider,
            "model.upsert",
            buildJsonObject {},
            objectOf("""{"model":{"ref":"new-provider/model-2"}}"""),
        )
        val deletedModel = RpcStateReducer.apply(
            upserted,
            "model.delete",
            buildJsonObject { put("ref", "new-provider/model-1") },
            buildJsonObject {},
        )
        val deletedProvider = RpcStateReducer.apply(
            deletedModel,
            "provider.delete",
            buildJsonObject { put("provider_id", "new-provider") },
            buildJsonObject {},
        )

        assertEquals(setOf("existing-skill", "imported-skill"), imported.skills.map { it.str("name") }.toSet())
        assertTrue(provider.providers.any { it.str("id") == "new-provider" })
        assertTrue(provider.models.any { it.str("ref") == "new-provider/model-1" })
        assertTrue(upserted.models.any { it.str("ref") == "new-provider/model-2" })
        assertTrue(deletedModel.models.none { it.str("ref") == "new-provider/model-1" })
        assertTrue(deletedProvider.providers.none { it.str("id") == "new-provider" })
    }
}

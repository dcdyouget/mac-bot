package bot.mac.mobile.core.protocol

import bot.mac.mobile.core.protocol.generated.*
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.*
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class FixtureContractTest {
    @Test
    fun everyFixtureUsesItsSchemaSerializerAndRoundTrips() {
        val corpus = fixtureCorpus()
        assertTrue(corpus.isNotEmpty())
        corpus.forEach { entry ->
            val source = entry.jsonObject.getValue("source").jsonPrimitive.content
            val value = entry.jsonObject.getValue("value")
            val serializer = assertNotNull(serializerFor(source), "No schema serializer for $source")
            assertEquals(value, roundTrip(serializer, value), source)
        }
    }

    @Test
    fun generatedAccessorsDecodeRepresentativeObjectsAndUnions() {
        val hello = decode(HelloSerializer, fixture("objects/hello.json"))
        assertEquals("node_1", hello.nodeId)
        assertEquals(1L, hello.protocol)

        val message = decode(MessageSerializer, fixture("objects/message.json"))
        assertEquals("msg_1", message.id)
        assertTrue(message.blocks.isNotEmpty())

        val event = decode(EventFrameSerializer, fixture("events/message_created.json"))
        assertEquals("message.created", event.event)
        assertEquals("msg_1", event.data.raw.obj("message").str("id"))

        val workbench = decode(WorkbenchSerializer, fixture("results/workbench.json"))
        assertEquals("bot_main", workbench.bots.single().botId)
        assertEquals("chat_login", workbench.bots.single().assignments.single().originChatId)
        assertEquals(0L, workbench.bots.single().assignments.single().pendingMessages)
        val queueReview = decode(TraceItemSerializer, fixture("trace/queue_review.json"))
        assertEquals("queue.review", queueReview.type)
        assertEquals(2L, queueReview.raw.obj("data").long("count"))

        val screen = decode(ScreenStateSerializer, fixture("frames/screen-state.json"))
        assertEquals("bot_main", screen.botId)
        assertEquals("tab_1", screen.tabs.single().tabId)

        val settingsRequest = decode(SettingsUpdateParamsSerializer, fixture("requests/settings_update_partial.json"))
        assertEquals(2L, settingsRequest.patch.concurrency?.global)
        assertEquals("inherit", settingsRequest.patch.models?.subagent)
        val searchSettings = decode(SettingsUpdateParamsSerializer, fixture("requests/settings_update_web_search.json"))
        assertNotNull(searchSettings.patch.webSearch)

        val approvalSettings = ApprovalSettings(fixture("objects/settings.json").obj("approvals"))
        assertEquals("always_allow", approvalSettings.mode)
        val builtInRule = approvalSettings.rules.first { it.builtin == true }
        assertEquals(null, builtInRule.botId)
        assertEquals("git push", builtInRule.text)
        assertEquals("bot_code", approvalSettings.rules.first { it.botId == "bot_code" }.botId)

        val permissions = decode(HostPermissionsResultSerializer, fixture("results/host_permissions.json"))
        assertEquals("granted", permissions.screenRecording)
        assertEquals("granted", permissions.automationChrome)

        val userMemory = decode(MemoryUserListResultSerializer, fixture("results/memory_user_list.json"))
        assertEquals("mem_system", userMemory.entries.first().id)
        assertEquals(1186L, userMemory.quota.usedChars)
        val userProfile = decode(MemoryProfileResultSerializer, fixture("results/memory_profile_get.json"))
        assertEquals("Asia/Shanghai", userProfile.profile.timezone)
    }

    @Test
    fun knownBlockFixturesDecodeAndUnknownBlockFallsBackLosslessly() {
        fixtureCorpus()
            .filter { path(it).startsWith("blocks/") }
            .forEach { entry -> assertIs<Block.Known>(Block.decode(entry.jsonObject.getValue("value").jsonObject)) }

        val unknownRaw = buildJsonObject {
            put("type", "future_block")
            put("fallback_text", "来自新服务端")
            put("extra", buildJsonObject { put("x", 1) })
        }
        val unknown = assertIs<Block.Unknown>(Block.decode(unknownRaw))
        assertEquals(unknownRaw, unknown.raw)
    }

    private fun fixtureCorpus(): JsonArray {
        val stream = requireNotNull(javaClass.getResourceAsStream("/protocol-fixtures.json")) {
            "protocol fixture corpus is missing; run python3 protocol/kotlin/generate.py"
        }
        return protocolJson.parseToJsonElement(stream.bufferedReader().use { it.readText() }).jsonArray
    }

    private fun fixture(source: String): JsonObject =
        fixtureCorpus().first { path(it) == source }
            .jsonObject.getValue("value").jsonObject

    private fun path(entry: JsonElement): String =
        entry.jsonObject.getValue("source").jsonPrimitive.content.removePrefix("protocol/fixtures/")

    private fun serializerFor(source: String): KSerializer<*>? {
        val path = source.removePrefix("protocol/fixtures/")
        return when {
        path.startsWith("errors/") -> RpcResponseSerializer
        path.startsWith("objects/") -> objectSerializers[path.substringAfter("objects/").substringBeforeLast('.')]
        path.startsWith("events/") || path.startsWith("scenarios/") -> EventFrameSerializer
        path.startsWith("requests/memory_") -> RpcRequestSerializer
        path == "results/memory_profile_get.json" || path == "results/memory_profile_update.json" -> MemoryProfileResultSerializer
        path == "results/memory_user_create.json" || path == "results/memory_user_update.json" -> MemoryUserEntryResultSerializer
        path == "results/memory_user_delete.json" -> MemoryUserDeleteResultSerializer
        path == "results/memory_user_list.json" -> MemoryUserListResultSerializer
        path == "requests/announcement_update.json" -> AnnouncementUpdateParamsSerializer
        path == "requests/artifact_version_get.json" -> ArtifactVersionGetParamsSerializer
        path == "requests/artifact_versions_list.json" -> ArtifactVersionsParamsSerializer
        path == "requests/assignment_retry.json" -> AssignmentRetryParamsSerializer
        path == "requests/chat_send_reference.json" -> ChatSendParamsSerializer
        path == "requests/message_delete.json" -> MessageDeleteParamsSerializer
        path == "requests/message_edit.json" -> MessageEditParamsSerializer
        path == "requests/message_steer.json" -> MessageSteerParamsSerializer
        path == "requests/question_list_history.json" -> QuestionListParamsSerializer
        path == "requests/host_permissions.json" -> JsonObject.serializer()
        path == "requests/settings_update_partial.json" || path == "requests/settings_update_web_search.json" -> SettingsUpdateParamsSerializer
        path.startsWith("trace/") -> TraceItemSerializer
        path == "frames/screen-header.json" -> ScreenFrameHeaderSerializer
        path == "frames/screen-state.json" -> ScreenStateSerializer
        path == "frames/screen-input.json" || path == "frames/screen-ack.json" -> ScreenClientFrameSerializer
        path == "results/workbench.json" -> WorkbenchSerializer
        path == "results/artifact_version_get.json" -> ArtifactVersionGetResultSerializer
        path == "results/artifact_versions_list.json" -> ArtifactVersionsResultSerializer
        path == "results/assignment_retry.json" -> AssignmentGetResultSerializer
        path == "results/host_permissions.json" -> HostPermissionsResultSerializer
        path == "results/message_delete.json" -> JsonObject.serializer()
        path == "results/message_edit.json" -> MessageEditResultSerializer
        path == "results/question_list_history.json" -> QuestionListResultSerializer
        path == "results/usage_heatmap_calendar.json" || path == "results/usage_heatmap_weekhour.json" -> HeatmapResultSerializer
        path.startsWith("blocks/") -> BlockSerializer
        else -> null
        }
    }

    private val objectSerializers: Map<String, KSerializer<*>> = mapOf(
        "announcement" to AnnouncementSerializer,
        "approval" to ApprovalSerializer,
        "artifact" to ArtifactSerializer,
        "artifact_version" to ArtifactVersionSerializer,
        "assignment" to AssignmentSerializer,
        "bot" to BotSerializer,
        "chat" to ChatSerializer,
        "device" to DeviceSerializer,
        "hello" to HelloSerializer,
        "memory_entry" to MemoryEntrySerializer,
        "memory_entry_500_chars" to MemoryEntrySerializer,
        "memory_quota" to MemoryQuotaSerializer,
        "memory_quota_full" to MemoryQuotaSerializer,
        "message" to MessageSerializer,
        "message_waiting" to MessageSerializer,
        "model" to ModelSerializer,
        "project" to ProjectSerializer,
        "provider" to ProviderSerializer,
        "question" to QuestionSerializer,
        "question_multi" to QuestionSerializer,
        "question_multi_answered" to QuestionSerializer,
        "routine" to RoutineSerializer,
        "routine_run" to RoutineRunSerializer,
        "settings" to SettingsSerializer,
        "skill" to SkillSerializer,
        "skill_draft" to SkillSerializer,
        "skill_detail" to SkillDetailSerializer,
        "user_profile" to UserProfileSerializer,
    )

    private fun <T> decode(serializer: KSerializer<T>, value: JsonObject): T =
        protocolJson.decodeFromJsonElement(serializer, value)

    @Suppress("UNCHECKED_CAST")
    private fun roundTrip(serializer: KSerializer<*>, value: JsonElement): JsonElement {
        val typed = serializer as KSerializer<Any>
        val decoded = protocolJson.decodeFromJsonElement(typed, value)
        return protocolJson.encodeToJsonElement(typed, decoded)
    }
}

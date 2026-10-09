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

        val screen = decode(ScreenStateSerializer, fixture("frames/screen-state.json"))
        assertEquals("bot_main", screen.botId)
        assertEquals("tab_1", screen.tabs.single().tabId)
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
        path.startsWith("objects/") -> objectSerializers[path.substringAfter("objects/").substringBeforeLast('.')]
        path.startsWith("events/") || path.startsWith("scenarios/") -> EventFrameSerializer
        path.startsWith("trace/") -> TraceItemSerializer
        path == "frames/screen-header.json" -> ScreenFrameHeaderSerializer
        path == "frames/screen-state.json" -> ScreenStateSerializer
        path == "frames/screen-input.json" || path == "frames/screen-ack.json" -> ScreenClientFrameSerializer
        path.startsWith("blocks/") -> BlockSerializer
        else -> null
        }
    }

    private val objectSerializers: Map<String, KSerializer<*>> = mapOf(
        "announcement" to AnnouncementSerializer,
        "approval" to ApprovalSerializer,
        "artifact" to ArtifactSerializer,
        "assignment" to AssignmentSerializer,
        "bot" to BotSerializer,
        "chat" to ChatSerializer,
        "device" to DeviceSerializer,
        "hello" to HelloSerializer,
        "message" to MessageSerializer,
        "model" to ModelSerializer,
        "project" to ProjectSerializer,
        "provider" to ProviderSerializer,
        "question" to QuestionSerializer,
        "routine" to RoutineSerializer,
        "routine_run" to RoutineRunSerializer,
        "settings" to SettingsSerializer,
        "skill" to SkillSerializer,
        "skill_detail" to SkillDetailSerializer,
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

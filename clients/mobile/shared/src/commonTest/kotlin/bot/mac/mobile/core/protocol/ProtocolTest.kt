package bot.mac.mobile.core.protocol
import kotlinx.serialization.json.*
import bot.mac.mobile.core.protocol.generated.UsageSeries
import bot.mac.mobile.core.protocol.generated.HeatmapResult
import kotlin.test.*
class ProtocolTest {
    @Test fun messageSourceMessageIdIsOptionalAndExposedWhenPresent() {
        assertNull(Message(buildJsonObject {}).sourceMessageId)
        val message = Message(protocolJson.parseToJsonElement("""{"source_message_id":"user-msg"}""").jsonObject)
        assertEquals("user-msg", message.sourceMessageId)
    }

    @Test fun nullableGeneratedFieldsPreserveNullAndValues() {
        for (raw in listOf(buildJsonObject {}, protocolJson.parseToJsonElement("""{"wait":null,"last_message":null,"quote":null,"answer":null,"calendar":null,"weekhour":null,"input_values":null,"output_values":null}""").jsonObject)) {
            assertNull(bot.mac.mobile.core.protocol.generated.Assignment(raw).wait)
            assertNull(bot.mac.mobile.core.protocol.generated.Chat(raw).lastMessage)
            assertNull(bot.mac.mobile.core.protocol.generated.Message(raw).quote)
            assertNull(bot.mac.mobile.core.protocol.generated.Question(raw).answer)
            assertNull(HeatmapResult(raw).calendar)
            assertNull(HeatmapResult(raw).weekhour)
            assertNull(UsageSeries(raw).inputValues)
            assertNull(UsageSeries(raw).outputValues)
        }
        val value = buildJsonObject { put("marker", "kept") }
        assertEquals(value, bot.mac.mobile.core.protocol.generated.Assignment(buildJsonObject { put("wait", value) }).wait?.raw)
        assertEquals(value, bot.mac.mobile.core.protocol.generated.Chat(buildJsonObject { put("last_message", value) }).lastMessage?.raw)
        assertEquals(value, bot.mac.mobile.core.protocol.generated.Message(buildJsonObject { put("quote", value) }).quote?.raw)
        assertEquals(value, bot.mac.mobile.core.protocol.generated.Question(buildJsonObject { put("answer", value) }).answer?.raw)
        val series = UsageSeries(protocolJson.parseToJsonElement("""{"input_values":[],"output_values":[1,2.5]}""").jsonObject)
        assertEquals(emptyList(), series.inputValues)
        assertEquals(listOf(1.0, 2.5), series.outputValues)
    }

    @Test fun unknownFieldsAndBlocksRoundTrip() {
        val source = protocolJson.parseToJsonElement("""{"id":"msg_1","extra":{"future":[1,null]},"blocks":[{"type":"future","payload":7}],"fallback_text":"fallback"}""").jsonObject
        val decoded = protocolJson.decodeFromJsonElement(WireObject.serializer(), source)
        assertIs<Block.Unknown>(decoded.blocks().single())
        assertEquals(source, protocolJson.encodeToJsonElement(WireObject.serializer(), decoded))
    }

    @Test fun typedModelsKeepUnknownFieldsAndExposeAccessors() {
        val source = protocolJson.parseToJsonElement(
            """{"protocol":1,"server_version":"0.1.0","node_id":"dev_1","host_name":"Mac","server_time":"2026-10-09T00:00:00Z","last_seq":4,"timezone":"Asia/Shanghai","currency":"CNY","features":["browser"],"future":{"keep":[1,null]}}"""
        ).jsonObject
        val decoded = protocolJson.decodeFromJsonElement(Hello.serializer(), source)
        assertEquals(1L, decoded.protocol)
        assertEquals("dev_1", decoded.nodeId)
        assertEquals(source, protocolJson.encodeToJsonElement(Hello.serializer(), decoded))
    }

    @Test fun unknownBlockRemainsUnknownInsideTypedMessage() {
        val source = protocolJson.parseToJsonElement(
            """{"id":"msg_1","chat_id":"chat_1","seq":1,"sender":{"kind":"user"},"created_at":"2026-10-09T00:00:00Z","edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[],"blocks":[{"type":"future","payload":{"x":1}}],"fallback_text":"future","intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[],"new_field":true}"""
        ).jsonObject
        val decoded = protocolJson.decodeFromJsonElement(Message.serializer(), source)
        assertIs<Block.Unknown>(decoded.blocks.single())
        assertEquals(source, protocolJson.encodeToJsonElement(Message.serializer(), decoded))
    }
}

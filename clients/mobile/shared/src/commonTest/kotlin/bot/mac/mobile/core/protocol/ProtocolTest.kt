package bot.mac.mobile.core.protocol
import kotlinx.serialization.json.*
import kotlin.test.*
class ProtocolTest {
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

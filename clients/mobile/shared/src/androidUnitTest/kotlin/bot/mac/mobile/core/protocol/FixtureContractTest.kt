package bot.mac.mobile.core.protocol
import kotlinx.serialization.json.*
import kotlin.test.*
class FixtureContractTest {
    @Test fun everyFixtureRoundTrips() {
        val stream = requireNotNull(javaClass.getResourceAsStream("/protocol-fixtures.json")) {
            "protocol fixture corpus is missing; run python3 protocol/kotlin/generate.py"
        }
        val corpus = protocolJson.parseToJsonElement(stream.bufferedReader().use { it.readText() }).jsonArray
        assertTrue(corpus.isNotEmpty())
        fun verify(value: JsonElement) {
            when (value) {
                is JsonObject -> {
                    val decoded = protocolJson.decodeFromJsonElement(WireObject.serializer(), value)
                    assertEquals(value, protocolJson.encodeToJsonElement(WireObject.serializer(), decoded))
                    value.values.forEach(::verify)
                }
                is JsonArray -> value.forEach(::verify)
                else -> Unit
            }
        }
        corpus.forEach { verify(it.jsonObject.getValue("value")) }
    }
}

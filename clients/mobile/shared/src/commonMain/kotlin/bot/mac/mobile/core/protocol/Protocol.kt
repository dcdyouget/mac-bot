package bot.mac.mobile.core.protocol

import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.*

val protocolJson = Json { ignoreUnknownKeys = true; explicitNulls = false }
fun JsonObject.str(key: String): String = (this[key] as? JsonPrimitive)?.contentOrNull.orEmpty()
fun JsonObject.long(key: String): Long = (this[key] as? JsonPrimitive)?.longOrNull ?: 0L
fun JsonObject.obj(key: String): JsonObject = this[key] as? JsonObject ?: buildJsonObject {}
fun JsonObject.arr(key: String): JsonArray = this[key] as? JsonArray ?: JsonArray(emptyList())
fun JsonObject.boolean(key: String): Boolean = (this[key] as? JsonPrimitive)?.booleanOrNull ?: false
fun JsonObject.objects(key: String): List<JsonObject> = arr(key).mapNotNull { it as? JsonObject }
fun jsonParams(vararg pairs: Pair<String, String>): JsonObject = buildJsonObject { pairs.forEach { put(it.first, it.second) } }

/** Keeps the complete wire object, including fields introduced by newer hosts. */
@Serializable(with = WireObjectSerializer::class)
data class WireObject(val raw: JsonObject) {
    val id get() = raw.str("id")
    fun blocks(): List<Block> = raw.objects("blocks").map { Block.decode(it) }
}
object WireObjectSerializer : KSerializer<WireObject> {
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
    override fun serialize(encoder: Encoder, value: WireObject) = encoder.encodeSerializableValue(JsonObject.serializer(), value.raw)
    override fun deserialize(decoder: Decoder) = WireObject(decoder.decodeSerializableValue(JsonObject.serializer()))
}
sealed class Block(open val raw: JsonObject) {
    data class Known(override val raw: JsonObject) : Block(raw)
    data class Unknown(override val raw: JsonObject) : Block(raw)
    val type get() = raw.str("type")
    companion object {
        val types = setOf("text", "image", "file", "task_card", "completion", "progress", "blocked", "question", "project_card", "review_card", "delegation", "approval", "approval_ref", "takeover_request", "bot_dm_ref", "system", "loop_paused")
        fun decode(raw: JsonObject): Block = if (raw.str("type") in types) Known(raw) else Unknown(raw)
    }
}

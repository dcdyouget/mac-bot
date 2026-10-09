package bot.mac.mobile.core.network

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

data class ScreenFrameHeader(
    val seq: Long,
    val tabId: String,
    val width: Int,
    val height: Int,
    val timestampMillis: Long,
    val url: String,
)

data class ScreenFrame(val header: ScreenFrameHeader, val jpeg: ByteArray) {
    override fun equals(other: Any?): Boolean = other is ScreenFrame && header == other.header && jpeg.contentEquals(other.jpeg)
    override fun hashCode(): Int = 31 * header.hashCode() + jpeg.contentHashCode()
}

class InvalidScreenFrame(message: String) : IllegalArgumentException(message)

object ScreenFrameCodec {
    private val json = Json { ignoreUnknownKeys = true }

    fun decode(bytes: ByteArray): ScreenFrame {
        if (bytes.size < 4) throw InvalidScreenFrame("screen frame has no header length")
        val headerLength = ((bytes[0].toInt() and 0xff) shl 24) or
            ((bytes[1].toInt() and 0xff) shl 16) or
            ((bytes[2].toInt() and 0xff) shl 8) or
            (bytes[3].toInt() and 0xff)
        if (headerLength < 2 || headerLength > bytes.size - 4) {
            throw InvalidScreenFrame("invalid screen header length: $headerLength")
        }
        val headerText = bytes.copyOfRange(4, 4 + headerLength).decodeToString()
        val obj = json.parseToJsonElement(headerText) as? JsonObject
            ?: throw InvalidScreenFrame("screen header is not an object")
        fun requiredString(name: String) = obj.string(name) ?: throw InvalidScreenFrame("missing $name")
        fun requiredLong(name: String) = obj.long(name) ?: throw InvalidScreenFrame("missing $name")
        val width = obj.long("w")?.toInt() ?: throw InvalidScreenFrame("missing w")
        val height = obj.long("h")?.toInt() ?: throw InvalidScreenFrame("missing h")
        return ScreenFrame(
            ScreenFrameHeader(requiredLong("seq"), requiredString("tab_id"), width, height,
                requiredLong("ts"), requiredString("url")),
            bytes.copyOfRange(4 + headerLength, bytes.size),
        )
    }

    fun ack(seq: Long): String = buildJsonObject {
        put("type", "ack")
        put("seq", seq)
    }.toString()

    fun switchTab(tabId: String): String = buildJsonObject {
        put("type", "switch_tab")
        put("tab_id", tabId)
    }.toString()
}

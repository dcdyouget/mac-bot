package bot.mac.mobile.core.network

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlinx.serialization.json.Json

class ScreenProtocolTest {
    @Test
    fun decodesBigEndianHeaderAndJpegPayload() {
        val header = "{\"seq\":7,\"tab_id\":\"tab-1\",\"w\":720,\"h\":480,\"ts\":123,\"url\":\"https://example.com\"}".encodeToByteArray()
        val jpeg = byteArrayOf(-1, -40, 1, 2, -1, -39)
        val frame = ByteArray(4 + header.size + jpeg.size)
        frame[0] = (header.size ushr 24).toByte()
        frame[1] = (header.size ushr 16).toByte()
        frame[2] = (header.size ushr 8).toByte()
        frame[3] = header.size.toByte()
        header.copyInto(frame, 4)
        jpeg.copyInto(frame, 4 + header.size)
        val decoded = ScreenFrameCodec.decode(frame)
        assertEquals(7L, decoded.header.seq)
        assertEquals(720, decoded.header.width)
        assertContentEquals(jpeg, decoded.jpeg)
    }

    @Test
    fun ackIsProtocolText() {
        val obj = Json.parseToJsonElement(ScreenFrameCodec.ack(9)).toString()
        assertEquals("{\"type\":\"ack\",\"seq\":9}", obj)
    }

    @Test
    fun rejectsEmptyPayloadAndInvalidDimensions() {
        fun frame(width: Int, payload: ByteArray): ByteArray {
            val header = """{"seq":1,"tab_id":"t","w":$width,"h":480,"ts":1,"url":""}""".encodeToByteArray()
            return byteArrayOf(0, 0, (header.size ushr 8).toByte(), header.size.toByte()) + header + payload
        }
        assertFailsWith<InvalidScreenFrame> { ScreenFrameCodec.decode(frame(720, byteArrayOf())) }
        assertFailsWith<InvalidScreenFrame> { ScreenFrameCodec.decode(frame(0, byteArrayOf(1))) }
    }

    @Test
    fun rejectsTruncatedFrame() {
        assertFailsWith<InvalidScreenFrame> { ScreenFrameCodec.decode(byteArrayOf(0, 0, 0, 20, 1)) }
    }
}

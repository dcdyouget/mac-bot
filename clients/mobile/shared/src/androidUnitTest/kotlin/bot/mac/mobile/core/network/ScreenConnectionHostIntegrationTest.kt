package bot.mac.mobile.core.network

import io.ktor.client.HttpClient
import io.ktor.client.engine.okhttp.OkHttp
import io.ktor.client.plugins.websocket.WebSockets
import bot.mac.mobile.core.protocol.protocolJson
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okio.ByteString
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ScreenConnectionHostIntegrationTest {
    @Test
    fun frameAckWaitsForRenderCallbackAndStopClosesSocket(): Unit = runBlocking {
        val server = MockWebServer()
        val opened = Channel<WebSocket>(Channel.UNLIMITED)
        val acks = Channel<JsonObject>(Channel.UNLIMITED)
        val closed = Channel<Unit>(Channel.UNLIMITED)
        val releaseRender = CompletableDeferred<Unit>()
        server.enqueue(MockResponse.Builder().webSocketUpgrade(ScreenListener(
            opened = opened,
            onText = { text ->
                if (text.contains("\"type\":\"ack\"")) acks.trySend(protocolJson.parseToJsonElement(text).jsonObject)
            },
            onClosed = { closed.trySend(Unit) },
            frame = encodedFrame(7, "tab-1"),
        )).build())
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val received = Channel<ScreenFrame>(Channel.UNLIMITED)
        val connection = ScreenConnection(
            client = client,
            host = HostProfile("host", "Host", listOf(server.url("/").toString()), "dev"),
            botId = "bot-1",
            quality = "high",
            scope = scope,
            onFrame = { frame ->
                received.send(frame)
                releaseRender.await()
            },
        )
        try {
            connection.start()
            withTimeout(2_000) { opened.receive() }
            val frame = withTimeout(2_000) { received.receive() }
            assertEquals(7L, frame.header.seq)
            assertEquals(null, withTimeoutOrNull(300) { acks.receive() })

            releaseRender.complete(Unit)
            assertEquals(7L, withTimeout(2_000) { acks.receive()["seq"].toString().trim('"').toLong() })

            connection.stop()
            withTimeout(2_000) { closed.receive() }
        } finally {
            scope.cancel()
            client.close()
            server.close()
        }
    }

    @Test
    fun switchTabIsSentAndRequestedTabIsUsedAfterReconnect(): Unit = runBlocking {
        val server = MockWebServer()
        val requestTabs = Channel<String?>(Channel.UNLIMITED)
        val switchTabs = Channel<String>(Channel.UNLIMITED)
        var connectionCount = 0
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse {
                requestTabs.trySend(request.url.queryParameter("tab_id"))
                connectionCount += 1
                return MockResponse.Builder().webSocketUpgrade(ScreenListener(
                    onText = { text ->
                        val value = runCatching { protocolJson.parseToJsonElement(text).jsonObject }.getOrNull()
                        if (value != null && value["type"]?.toString()?.trim('"') == "switch_tab") {
                            switchTabs.trySend(value["tab_id"]?.toString()?.trim('"').orEmpty())
                        }
                    },
                    closeOnSwitch = connectionCount == 1,
                )).build()
            }
        }
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val connection = ScreenConnection(
            client = client,
            host = HostProfile("host", "Host", listOf(server.url("/").toString()), "dev"),
            botId = "bot-1",
            quality = "auto",
            scope = scope,
        )
        try {
            connection.start()
            assertEquals(null, withTimeout(2_000) { requestTabs.receive() })
            withTimeout(2_000) {
                while (!connection.switchTab("tab-2")) delay(10)
            }
            assertEquals("tab-2", withTimeout(2_000) { switchTabs.receive() })
            assertEquals("tab-2", withTimeout(5_000) { requestTabs.receive() })
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.close()
        }
    }

    private class ScreenListener(
        private val opened: Channel<WebSocket>? = null,
        private val onText: (String) -> Unit = {},
        private val onClosed: () -> Unit = {},
        private val frame: ByteArray? = null,
        private val closeOnSwitch: Boolean = false,
    ) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: okhttp3.Response) {
            opened?.trySend(webSocket)
            webSocket.send(stateFrame())
            frame?.let { webSocket.send(ByteString.of(*it)) }
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            onText(text)
            if (closeOnSwitch && text.contains("\"type\":\"switch_tab\"")) webSocket.close(1000, "switch")
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) = Unit

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, reason)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            onClosed()
        }

        private fun stateFrame(): String = buildJsonObject {
            put("type", "state")
            put("state", buildJsonObject {
                put("bot_id", "bot-1")
                put("driver", "bot")
                put("width", 1280)
                put("height", 720)
                put("tabs", buildJsonArray { })
            })
        }.toString()
    }

    private fun encodedFrame(seq: Long, tabId: String): ByteArray {
        val header = buildJsonObject {
            put("seq", seq)
            put("tab_id", tabId)
            put("w", 1280)
            put("h", 720)
            put("ts", 1_791_537_542_312L)
            put("url", "http://localhost")
        }.toString().encodeToByteArray()
        val jpeg = byteArrayOf(0xff.toByte(), 0xd8.toByte(), 1, 2, 0xff.toByte(), 0xd9.toByte())
        return ByteArray(4 + header.size + jpeg.size).also { bytes ->
            bytes[0] = (header.size ushr 24).toByte()
            bytes[1] = (header.size ushr 16).toByte()
            bytes[2] = (header.size ushr 8).toByte()
            bytes[3] = header.size.toByte()
            header.copyInto(bytes, 4)
            jpeg.copyInto(bytes, 4 + header.size)
        }
    }
}

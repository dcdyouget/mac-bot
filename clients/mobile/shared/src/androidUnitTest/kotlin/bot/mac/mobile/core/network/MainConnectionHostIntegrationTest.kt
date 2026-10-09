package bot.mac.mobile.core.network

import io.ktor.client.HttpClient
import io.ktor.client.engine.okhttp.OkHttp
import io.ktor.client.plugins.websocket.WebSockets
import kotlinx.coroutines.Channel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okio.ByteString
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * Host-side contract tests for the real Ktor/OkHttp WebSocket path.
 *
 * These intentionally exercise MainConnection rather than RequestTracker in isolation:
 * the server closes/reorders sockets and emits events at the handshake boundary.
 */
class MainConnectionHostIntegrationTest {
    private val json = Json { ignoreUnknownKeys = true }

    @Test
    fun bootstrapResponseRaceFlushesPersistentEventAndCursor() = runBlocking {
        val server = MockWebServer()
        server.enqueue(upgrade { socket, request ->
            when (request.string("method")) {
                "session.resume" -> socket.send(response(request.id(), buildJsonObject { put("mode", "reset") }))
                "bootstrap" -> {
                    socket.send(response(request.id(), buildJsonObject {
                        put("seq", 4)
                        put("snapshot", buildJsonObject { put("version", 1) })
                    }))
                    // This is deliberately sent immediately after the bootstrap response.
                    socket.send(event("message.created", buildJsonObject { put("id", "m1") }, 5))
                }
            }
        })
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val store = InMemoryLastSeqStore()
        val received = Channel<MainEvent>(Channel.UNLIMITED)
        val connection = MainConnection(
            client = client,
            host = host(server),
            identity = identity(),
            lastSeqStore = store,
            scope = scope,
            onEvent = { received.send(it) },
        )
        try {
            connection.start()
            assertEquals(ConnectionStatus.CONNECTED, withTimeout(5_000) {
                connection.status.first { it == ConnectionStatus.CONNECTED }
            })
            val hello = withTimeout(2_000) { received.receive() }
            val bootstrap = withTimeout(2_000) { received.receive() }
            val persistent = withTimeout(2_000) { received.receive() }
            assertTrue(hello is MainEvent.Hello)
            assertTrue(bootstrap is MainEvent.Ephemeral && bootstrap.event == "bootstrap")
            assertTrue(persistent is MainEvent.Persistent && persistent.seq == 5L)
            withTimeout(2_000) { connection.snapshot.first { it.lastSeq == 5L } }
            assertEquals(5L, store.get("integration"))
            assertEquals(5L, connection.snapshot.value.lastSeq)
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.shutdown()
        }
    }

    @Test
    fun reconnectReplaysWriteWithStableClientRequestId() = runBlocking {
        val server = MockWebServer()
        val firstRequest = Channel<JsonObject>(Channel.UNLIMITED)
        val secondRequest = Channel<JsonObject>(Channel.UNLIMITED)
        server.enqueue(upgrade { socket, request ->
            when (request.string("method")) {
                "session.resume" -> socket.send(response(request.id(), buildJsonObject { put("mode", "reset") }))
                "bootstrap" -> socket.send(response(request.id(), buildJsonObject { put("seq", 0) }))
                "write.once" -> {
                    firstRequest.trySend(request)
                    socket.close(1001, "replay")
                }
            }
        })
        server.enqueue(upgrade { socket, request ->
            when (request.string("method")) {
                "session.resume" -> {
                    socket.send(response(request.id(), buildJsonObject { put("mode", "replay") }))
                    socket.send(event("sync.done", buildJsonObject { put("seq", 0) }, 0))
                }
                "write.once" -> {
                    secondRequest.trySend(request)
                    socket.send(response(request.id(), buildJsonObject { put("replayed", true) }))
                }
            }
        })
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val connection = MainConnection(
            client, host(server), identity(), InMemoryLastSeqStore(), scope,
        )
        try {
            connection.start()
            withTimeout(5_000) { connection.status.first { it == ConnectionStatus.CONNECTED } }
            val result = withTimeout(8_000) {
                connection.request(
                    "write.once",
                    buildJsonObject { put("payload", "once") },
                    write = true,
                )
            }
            val first = withTimeout(2_000) { firstRequest.receive() }
            val second = withTimeout(2_000) { secondRequest.receive() }
            assertEquals(true, result["replayed"]?.jsonPrimitive?.content?.toBoolean())
            assertEquals(first.id(), second.id())
            assertEquals(first.params().string("client_request_id"), second.params().string("client_request_id"))
            assertNotNull(first.params().string("client_request_id"))
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.shutdown()
        }
    }

    @Test
    fun concurrentResponsesAreMatchedByRequestId() = runBlocking {
        val server = MockWebServer()
        val requests = mutableListOf<JsonObject>()
        val requestGate = Channel<Unit>(Channel.UNLIMITED)
        server.enqueue(upgrade { socket, request ->
            when (request.string("method")) {
                "session.resume" -> {
                    socket.send(response(request.id(), buildJsonObject { put("mode", "replay") }))
                    socket.send(event("sync.done", buildJsonObject { put("seq", 0) }, 0))
                }
                "echo" -> synchronized(requests) {
                    requests += request
                    if (requests.size == 2) {
                        val second = requests[1]
                        val first = requests[0]
                        socket.send(response(second.id(), buildJsonObject { put("label", second.params().string("label")) }))
                        socket.send(response(first.id(), buildJsonObject { put("label", first.params().string("label")) }))
                        requestGate.trySend(Unit)
                    }
                }
            }
        })
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val connection = MainConnection(client, host(server), identity(), InMemoryLastSeqStore(), scope)
        try {
            connection.start()
            withTimeout(5_000) { connection.status.first { it == ConnectionStatus.CONNECTED } }
            val results = listOf("a", "b").map { label ->
                async {
                    connection.request("echo", buildJsonObject { put("label", label) })
                        .string("label")
                }
            }.awaitAll()
            withTimeout(2_000) { requestGate.receive() }
            assertEquals(setOf("a", "b"), results.toSet())
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.shutdown()
        }
    }

    @Test
    fun failingPersistentHandlerDoesNotAdvanceCursor() = runBlocking {
        val server = MockWebServer()
        val persistentSeen = Channel<Unit>(Channel.UNLIMITED)
        server.enqueue(upgrade { socket, request ->
            when (request.string("method")) {
                "session.resume" -> socket.send(response(request.id(), buildJsonObject { put("mode", "reset") }))
                "bootstrap" -> {
                    socket.send(response(request.id(), buildJsonObject { put("seq", 0) }))
                    socket.send(event("message.created", buildJsonObject { put("id", "failed") }, 1))
                }
            }
        })
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val store = InMemoryLastSeqStore()
        val connection = MainConnection(
            client, host(server), identity(), store, scope,
            onEvent = { if (it is MainEvent.Persistent) {
                persistentSeen.trySend(Unit)
                error("state reducer failed")
            } },
        )
        try {
            connection.start()
            withTimeout(5_000) { persistentSeen.receive() }
            assertEquals(0L, store.get("integration"))
            assertEquals(0L, connection.snapshot.value.lastSeq)
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.shutdown()
        }
    }

    private fun upgrade(handler: (WebSocket, JsonObject) -> Unit): MockResponse =
        MockResponse().withWebSocketUpgrade(ScriptListener(json, handler))

    private fun host(server: MockWebServer) = HostProfile(
        id = "integration",
        name = "Integration",
        addresses = listOf(server.url("/").toString()),
        password = "dev",
    )

    private fun identity() = ClientIdentity(
        appVersion = "test",
        deviceName = "host-test",
        deviceId = "host-test-device",
    )

    private fun event(name: String, data: JsonObject, seq: Long? = null): String =
        buildJsonObject {
            put("v", 1)
            put("kind", "evt")
            if (seq != null) put("seq", seq)
            put("event", name)
            put("data", data)
        }.toString()

    private fun response(id: String, result: JsonObject): String =
        buildJsonObject {
            put("v", 1)
            put("kind", "res")
            put("id", id)
            put("ok", true)
            put("result", result)
        }.toString()

    private class ScriptListener(
        private val json: Json,
        private val handler: (WebSocket, JsonObject) -> Unit,
    ) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: okhttp3.Response) {
            webSocket.send(helloFrame())
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val request = runCatching { json.parseToJsonElement(text).jsonObject }.getOrNull() ?: return
            handler(webSocket, request)
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) = Unit

        private fun helloFrame(): String = buildJsonObject {
            put("v", 1)
            put("kind", "evt")
            put("event", "hello")
            put("data", buildJsonObject {
                put("protocol", 1)
                put("node_id", "node-test")
                put("server_version", "test")
            })
        }.toString()
    }

    private fun JsonObject.string(name: String): String? = this[name]?.toString()?.trim('"')
    private fun JsonObject.id(): String = string("id") ?: error("missing id")
    private fun JsonObject.params(): JsonObject = this["params"]?.jsonObject ?: buildJsonObject { }
}

package bot.mac.mobile.core.network

import io.ktor.client.HttpClient
import io.ktor.client.engine.okhttp.OkHttp
import io.ktor.client.plugins.websocket.WebSockets
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
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
    fun bootstrapResponseRaceFlushesPersistentEventAndCursor() {
        runBlocking {
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
            server.close()
        }
        }
    }

    @Test
    fun replayDeduplicatesPersistentSequencesAndKeepsEphemeralOrder() {
        runBlocking {
            val server = MockWebServer()
            server.enqueue(upgrade { socket, request ->
                when (request.string("method")) {
                    "session.resume" -> {
                        socket.send(response(request.id(), buildJsonObject { put("mode", "replay") }))
                        socket.send(event("persistent", buildJsonObject { put("label", "three") }, 3))
                        socket.send(event("ephemeral", buildJsonObject { put("label", "first") }))
                        socket.send(event("persistent", buildJsonObject { put("label", "two") }, 2))
                        socket.send(event("persistent", buildJsonObject { put("label", "duplicate") }, 2))
                        socket.send(event("ephemeral", buildJsonObject { put("label", "second") }))
                        socket.send(event("sync.done", buildJsonObject { put("seq", 3) }, 3))
                    }
                }
            })
            server.start()
            val client = HttpClient(OkHttp) { install(WebSockets) }
            val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val store = InMemoryLastSeqStore()
            val received = Channel<MainEvent>(Channel.UNLIMITED)
            val connection = MainConnection(
                client, host(server), identity(), store, scope,
                onEvent = { received.send(it) },
            )
            try {
                connection.start()
                withTimeout(5_000) { connection.status.first { it == ConnectionStatus.CONNECTED } }
                val events = withTimeout(2_000) { buildList { repeat(5) { add(received.receive()) } } }
                assertTrue(events[0] is MainEvent.Hello)
                assertEquals(
                    listOf("two", "three", "first", "second"),
                    events.drop(1).map { event ->
                        when (event) {
                            is MainEvent.Persistent -> event.data.string("label")
                            is MainEvent.Ephemeral -> event.data.string("label")
                            else -> error("unexpected replay event: $event")
                        }
                    },
                )
                assertEquals(3L, store.get("integration"))
            } finally {
                connection.stop()
                scope.cancel()
                client.close()
                server.close()
            }
        }
    }

    @Test
    fun reconnectReplaysWriteWithStableClientRequestId() {
        runBlocking {
        val server = MockWebServer()
        val firstRequest = Channel<JsonObject>(Channel.UNLIMITED)
        val secondRequest = Channel<JsonObject>(Channel.UNLIMITED)
        var connectionCount = 0
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse {
                connectionCount += 1
                val firstConnection = connectionCount == 1
                return upgrade { socket, frame ->
                    when (frame.string("method")) {
                        "session.resume" -> {
                            val mode = if (firstConnection) "reset" else "replay"
                            socket.send(response(frame.id(), buildJsonObject { put("mode", mode) }))
                            if (!firstConnection) socket.send(event("sync.done", buildJsonObject { put("seq", 0) }, 0))
                        }
                        "bootstrap" -> socket.send(response(frame.id(), buildJsonObject { put("seq", 0) }))
                        "chat.send" -> {
                            if (firstConnection) {
                                firstRequest.trySend(frame)
                                socket.close(1001, "replay")
                            } else {
                                secondRequest.trySend(frame)
                                socket.send(response(frame.id(), buildJsonObject { put("replayed", true) }))
                            }
                        }
                    }
                }
            }
        }
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
                    "chat.send",
                    buildJsonObject {
                        put("chat_id", "chat-1")
                        put("text", "hello")
                        put("client_locale", "zh-Hant-TW")
                    },
                    write = true,
                )
            }
            val first = withTimeout(2_000) { firstRequest.receive() }
            val second = withTimeout(2_000) { secondRequest.receive() }
            assertEquals(true, result["replayed"]?.jsonPrimitive?.content?.toBoolean())
            assertEquals(first.id(), second.id())
            assertEquals(first.params().string("client_request_id"), second.params().string("client_request_id"))
            assertNotNull(first.params().string("client_request_id"))
            assertEquals("zh-Hant-TW", first.params().string("client_locale"))
            assertEquals(first.params().string("client_locale"), second.params().string("client_locale"))
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.close()
        }
        }
    }

    @Test
    fun concurrentResponsesAreMatchedByRequestId() {
        runBlocking {
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
            server.close()
        }
        }
    }

    @Test
    fun rpcHeartbeatKeepsOkHttpConnectionAliveAndRequestsUsable() {
        runBlocking {
        val server = MockWebServer()
        val pings = Channel<Unit>(Channel.UNLIMITED)
        server.enqueue(upgrade { socket, request ->
            when (request.string("method")) {
                "session.resume" -> {
                    socket.send(response(request.id(), buildJsonObject { put("mode", "replay") }))
                    socket.send(event("sync.done", buildJsonObject { put("seq", 0) }, 0))
                }
                "ping" -> {
                    pings.trySend(Unit)
                    socket.send(response(request.id(), buildJsonObject { put("server_time", "2026-10-09T00:00:00Z") }))
                }
                "echo" -> socket.send(response(request.id(), buildJsonObject { put("ok", true) }))
            }
        })
        server.start()
        val client = HttpClient(OkHttp) { install(WebSockets) }
        val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val connection = MainConnection(
            client = client,
            host = host(server),
            identity = identity(),
            lastSeqStore = InMemoryLastSeqStore(),
            scope = scope,
            heartbeatMillis = 100L,
        )
        try {
            connection.start()
            withTimeout(5_000) { connection.status.first { it == ConnectionStatus.CONNECTED } }
            repeat(3) { withTimeout(2_000) { pings.receive() } }
            assertEquals(ConnectionStatus.CONNECTED, connection.status.value)
            val result = withTimeout(2_000) {
                connection.request("echo", buildJsonObject { put("value", "after-heartbeat") })
            }
            assertEquals("true", result["ok"]?.jsonPrimitive?.content)
        } finally {
            connection.stop()
            scope.cancel()
            client.close()
            server.close()
        }
        }
    }

    @Test
    fun failingPersistentHandlerDoesNotAdvanceCursor() {
        runBlocking {
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
            server.close()
        }
    }
    }

    @Test
    fun handshakeTimeoutRetriesWithoutSwallowingRunnerCancellation() {
        runBlocking {
            val server = MockWebServer()
            val firstResumeReceived = Channel<Unit>(Channel.UNLIMITED)
            var connectionCount = 0
            server.dispatcher = object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse {
                    connectionCount += 1
                    val leaveResumeUnanswered = connectionCount == 1
                    return upgrade { socket, frame ->
                        if (frame.string("method") == "session.resume") {
                            if (leaveResumeUnanswered) {
                                firstResumeReceived.trySend(Unit)
                            } else {
                                socket.send(response(frame.id(), buildJsonObject { put("mode", "replay") }))
                                socket.send(event("sync.done", buildJsonObject { put("seq", 0) }, 0))
                            }
                        }
                    }
                }
            }
            server.start()
            val client = HttpClient(OkHttp) { install(WebSockets) }
            val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val diagnostics = Channel<ConnectionDiagnostic>(Channel.UNLIMITED)
            val connection = MainConnection(
                client = client,
                host = host(server),
                identity = identity(),
                lastSeqStore = InMemoryLastSeqStore(),
                scope = scope,
                // The first resume request is intentionally unanswered. Leave enough
                // headroom for the successful retry to complete on a loaded test host.
                requestTimeoutMillis = 2_000L,
                onDiagnostic = { diagnostic -> diagnostics.trySend(diagnostic) },
            )
            try {
                connection.start()
                withTimeout(10_000) { firstResumeReceived.receive() }
                var timeoutFailure: ConnectionDiagnostic? = null
                withTimeout(5_000) {
                    while (timeoutFailure == null) {
                        val diagnostic = diagnostics.receive()
                        if (diagnostic.stage == "session_resume" && diagnostic.exceptionClass != null) {
                            timeoutFailure = diagnostic
                        }
                    }
                }
                assertTrue(timeoutFailure?.exceptionClass?.contains("Timeout") == true)
                val connected = withTimeoutOrNull(10_000) {
                    connection.status.first { it == ConnectionStatus.CONNECTED }
                }
                assertTrue(
                    connected == ConnectionStatus.CONNECTED,
                    "status=${connection.status.value}, requests=${server.requestCount}, diagnostics=${diagnostics.drain()}",
                )
            } finally {
                connection.stop()
                scope.cancel()
                client.close()
                server.close()
            }
        }
    }

    @Test
    fun rejectedWebSocketReportsOnlyStageStatusAndExceptionClass() {
        runBlocking {
            val server = MockWebServer()
            val rejectedHandshakeReceived = CompletableDeferred<Unit>()
            server.dispatcher = object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse {
                    rejectedHandshakeReceived.complete(Unit)
                    return MockResponse.Builder()
                        .code(401)
                        .addHeader("Connection", "close")
                        .build()
                }
            }
            server.start()
            val client = HttpClient(OkHttp) { install(WebSockets) }
            val scope = kotlinx.coroutines.CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val diagnostics = Channel<ConnectionDiagnostic>(Channel.UNLIMITED)
            val connection = MainConnection(
                client = client,
                host = host(server).copy(password = "diagnostic-secret"),
                identity = identity(),
                lastSeqStore = InMemoryLastSeqStore(),
                scope = scope,
                onDiagnostic = { diagnostic -> diagnostics.trySend(diagnostic) },
            )
            try {
                connection.start()
                assertEquals("websocket_open", withTimeout(2_000) { diagnostics.receive() }.stage)
                // The first diagnostic is emitted before the HTTP request begins. Wait for the
                // mock to receive the rejected upgrade before asserting the transport failure.
                withTimeout(5_000) { rejectedHandshakeReceived.await() }
                var failed: ConnectionDiagnostic? = null
                withTimeout(5_000) {
                    while (failed == null) {
                        diagnostics.receive().let { diagnostic ->
                            if (diagnostic.exceptionClass != null) failed = diagnostic
                        }
                    }
                }
                val failure = failed ?: error("missing rejected handshake diagnostic")
                assertEquals("websocket_open", failure.stage)
                // OkHttp/Ktor may expose the rejected handshake as WebSocketException
                // without retaining the HTTP response; when it is retained, keep 401.
                assertTrue(failure.httpStatus == null || failure.httpStatus == 401)
                val exceptionClass = failure.exceptionClass ?: error("missing exception class")
                assertTrue(exceptionClass.isNotBlank())
                assertTrue(exceptionClass.contains("Exception"))
                assertTrue(failure.toString().contains("websocket_open"))
                assertTrue(!failure.toString().contains("diagnostic-secret"))
                assertTrue(!failure.toString().contains(server.url("/").toString()))
            } finally {
                connection.stop()
                scope.cancel()
                client.close()
                server.close()
            }
        }
    }

    private fun upgrade(handler: (WebSocket, JsonObject) -> Unit): MockResponse =
        MockResponse.Builder().webSocketUpgrade(ScriptListener(json, handler)).build()

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

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, reason)
        }

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

    private fun Channel<ConnectionDiagnostic>.drain(): List<ConnectionDiagnostic> = buildList {
        while (true) add(tryReceive().getOrNull() ?: break)
    }
}

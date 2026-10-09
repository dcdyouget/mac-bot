package bot.mac.mobile.core.state

import bot.mac.mobile.core.network.ConnectionStatus
import bot.mac.mobile.core.network.MainEvent
import bot.mac.mobile.core.platform.CredentialStore
import bot.mac.mobile.core.platform.PersistentStore
import bot.mac.mobile.core.protocol.protocolJson
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.yield
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.jsonObject
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okio.ByteString
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicBoolean

class ClientRepositoryMultiHostTest {
    @Test
    fun identicalConcurrentWritesLeaseDistinctClientRequestIds(): Unit = runBlocking {
        val hostA = TestHost("node-a")
        val hostB = TestHost("node-b")
        val repository = repository(hostA, hostB)
        try {
            awaitConnected(repository, "a", "b")
            hostA.blockChatSends = true
            val params = buildJsonObject {
                put("chat_id", "chat_same")
                put("text", "same payload")
                put("mentions", kotlinx.serialization.json.buildJsonArray {})
            }
            val first = async { repository.callOnHost("a", "chat.send", params) }
            val second = async { repository.callOnHost("a", "chat.send", params) }
            val requests = listOf(
                withTimeout(2_000) { hostA.writeRequests.receive() },
                withTimeout(2_000) { hostA.writeRequests.receive() },
            )
            assertNotEquals(
                requests[0].obj("params").str("client_request_id"),
                requests[1].obj("params").str("client_request_id"),
            )
            hostA.releaseChatSends.countDown()
            first.await()
            second.await()
        } finally {
            hostA.releaseChatSends.countDown()
            repository.scope.cancel()
            hostA.close()
            hostB.close()
        }
    }

    @Test
    fun failedWriteKeepsItsClientRequestIdForTheNextRetry() = runBlocking {
        val hostA = TestHost("node-a")
        val hostB = TestHost("node-b")
        val repository = repository(hostA, hostB)
        try {
            awaitConnected(repository, "a", "b")
            hostA.failNextChatSend.set(true)
            val params = buildJsonObject {
                put("chat_id", "chat_retry")
                put("text", "retry me")
                put("mentions", kotlinx.serialization.json.buildJsonArray {})
            }
            runCatching { repository.callOnHost("a", "chat.send", params) }
                .onSuccess { error("first write unexpectedly succeeded") }
            repository.callOnHost("a", "chat.send", params)
            val first = withTimeout(2_000) { hostA.writeRequests.receive() }
            val second = withTimeout(2_000) { hostA.writeRequests.receive() }
            assertEquals(
                first.obj("params").str("client_request_id"),
                second.obj("params").str("client_request_id"),
            )
        } finally {
            repository.scope.cancel()
            hostA.close()
            hostB.close()
        }
    }

    @Test
    fun cancelledWriteReleasesItsLeaseForTheNextRetry() = runBlocking {
        val hostA = TestHost("node-a")
        val hostB = TestHost("node-b")
        val repository = repository(hostA, hostB)
        try {
            awaitConnected(repository, "a", "b")
            hostA.blockChatSends = true
            val params = buildJsonObject {
                put("chat_id", "chat_cancel")
                put("text", "cancel me")
                put("mentions", kotlinx.serialization.json.buildJsonArray {})
            }
            val cancelled = async { repository.callOnHost("a", "chat.send", params) }
            val first = withTimeout(2_000) { hostA.writeRequests.receive() }
            cancelled.cancelAndJoin()
            hostA.blockChatSends = false
            repository.callOnHost("a", "chat.send", params)
            val second = withTimeout(2_000) { hostA.writeRequests.receive() }
            assertEquals(
                first.obj("params").str("client_request_id"),
                second.obj("params").str("client_request_id"),
            )
        } finally {
            hostA.releaseChatSends.countDown()
            repository.scope.cancel()
            hostA.close()
            hostB.close()
        }
    }

    @Test
    fun inactiveHostEventsStayScopedAndSelectingDoesNotDisconnectOtherHosts(): Unit = runBlocking {
        val hostA = TestHost("node-a")
        val hostB = TestHost("node-b")
        val repository = repository(hostA, hostB)
        try {
            awaitConnected(repository, "a", "b")
            assertEquals("a", repository.activeHost.value?.id)
            val socketB = withTimeout(2_000) { hostB.sockets.receive() }

            val eventSeen = async {
                withTimeout(2_000) {
                    repository.hostEvents.first { it.hostId == "b" && it.event is MainEvent.Persistent }
                }
            }
            yield()
            socketB.send(event("bot.updated", buildJsonObject {
                put("bot", buildJsonObject { put("id", "bot-b"); put("name", "B") })
            }, 1))
            eventSeen.await()
            assertTrue(repository.hostStates.value.getValue("b").bots.any { it["id"].toString().contains("bot-b") })
            assertFalse(repository.state.value.bots.any { it["id"].toString().contains("bot-b") })
            repository.selectHost("b")
            assertTrue(repository.state.value.bots.any { it["id"].toString().contains("bot-b") })
            assertEquals(ConnectionStatus.CONNECTED, repository.hostStatuses.value.getValue("a").status)
            assertEquals(ConnectionStatus.CONNECTED, repository.hostStatuses.value.getValue("b").status)
        } finally {
            repository.scope.cancel()
            hostA.close()
            hostB.close()
        }
    }

    @Test
    fun deleteAndReaddHostInvalidateOldGenerationCallbacks(): Unit = runBlocking {
        val hostA = TestHost("node-a")
        val hostB = TestHost("node-b")
        val repository = repository(hostA, hostB)
        val observed = Channel<HostEvent>(Channel.UNLIMITED)
        val observer = CoroutineScope(SupervisorJob() + Dispatchers.Default).launch {
            repository.hostEvents.collect {
                if (it.hostId == "a" && (it.event as? MainEvent.Persistent)?.seq == 99L) observed.send(it)
            }
        }
        try {
            awaitConnected(repository, "a", "b")
            repository.selectHost("b")
            val oldSocketA = withTimeout(2_000) { hostA.sockets.receive() }

            repository.deleteHost("a")
            assertFalse(repository.hostStatuses.value.containsKey("a"))
            assertFalse(repository.hostStates.value.containsKey("a"))
            assertEquals("b", repository.activeHost.value?.id)

            oldSocketA.send(event("bot.updated", buildJsonObject {
                put("bot", buildJsonObject { put("id", "stale-a") })
            }, 99))
            assertEquals(null, withTimeoutOrNull(500) { observed.receive() })

            hostA.enqueueConnection()
            repository.saveHost("A", listOf(hostA.address()), "dev", existingId = "a")
            awaitConnected(repository, "a", "b")
            repository.selectHost("b")
            assertFalse(repository.hostStates.value.getValue("b").bots.any { it["id"].toString().contains("stale-a") })
            assertFalse(repository.hostStates.value.getValue("a").bots.any { it["id"].toString().contains("stale-a") })
        } finally {
            observer.cancel()
            repository.scope.cancel()
            hostA.close()
            hostB.close()
        }
    }

    private suspend fun repository(hostA: TestHost, hostB: TestHost): ClientRepository {
        val storage = MemoryPersistentStore()
        val credentials = MemoryCredentialStore(mapOf("a" to "dev", "b" to "dev"))
        storage.write("host_records", protocolJson.encodeToString(listOf(
            SavedHost("a", "A", listOf(hostA.address())),
            SavedHost("b", "B", listOf(hostB.address())),
        )))
        val repository = ClientRepository(storage, credentials)
        repository.initialize()
        return repository
    }

    private suspend fun awaitConnected(repository: ClientRepository, vararg ids: String) {
        withTimeout(5_000) {
            repository.hostStatuses.first { statuses ->
                ids.all { statuses[it]?.status == ConnectionStatus.CONNECTED }
            }
        }
    }

    private fun event(name: String, data: JsonObject, seq: Long): String = buildJsonObject {
        put("v", 1)
        put("kind", "evt")
        put("seq", seq)
        put("event", name)
        put("data", data)
    }.toString()

    private class TestHost(private val nodeId: String) {
        val server = MockWebServer()
        val sockets = Channel<WebSocket>(Channel.UNLIMITED)
        val writeRequests = Channel<JsonObject>(Channel.UNLIMITED)
        var blockChatSends = false
        val releaseChatSends = CountDownLatch(1)
        val failNextChatSend = AtomicBoolean(false)
        private val allSockets = mutableListOf<WebSocket>()

        init {
            enqueueConnection()
            server.start()
        }

        fun enqueueConnection() {
            server.enqueue(MockResponse.Builder().webSocketUpgrade(Listener(nodeId, sockets, allSockets, this)).build())
        }

        fun address(): String = server.url("/").toString()

        fun close() {
            synchronized(allSockets) {
                allSockets.forEach { socket ->
                    runCatching { socket.close(1000, "test cleanup") }
                }
                allSockets.clear()
            }
            sockets.close()
            runCatching { server.close() }
        }
    }

    private class Listener(
        private val nodeId: String,
        private val sockets: Channel<WebSocket>,
        private val allSockets: MutableList<WebSocket>,
        private val host: TestHost,
    ) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: okhttp3.Response) {
            sockets.trySend(webSocket)
            synchronized(allSockets) { allSockets += webSocket }
            webSocket.send(buildJsonObject {
                put("v", 1)
                put("kind", "evt")
                put("event", "hello")
                put("data", buildJsonObject {
                    put("protocol", 1)
                    put("node_id", nodeId)
                    put("server_version", "test")
                })
            }.toString())
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val request = runCatching { protocolJson.parseToJsonElement(text).jsonObject }.getOrNull() ?: return
            val id = request["id"]?.toString()?.trim('"') ?: return
            when (request["method"]?.toString()?.trim('"')) {
                "session.resume" -> webSocket.send(response(id, buildJsonObject { put("mode", "reset") }))
                "bootstrap" -> webSocket.send(response(id, buildJsonObject { put("seq", 0) }))
                "chat.send" -> {
                    host.writeRequests.trySend(request)
                    if (host.failNextChatSend.compareAndSet(true, false)) {
                        webSocket.send(errorResponse(id))
                    } else if (host.blockChatSends) {
                        Thread {
                            host.releaseChatSends.await()
                            webSocket.send(response(id, buildJsonObject {}))
                        }.start()
                    } else {
                        webSocket.send(response(id, buildJsonObject {}))
                    }
                }
                else -> webSocket.send(response(id, buildJsonObject {}))
            }
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) = Unit

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, reason)
        }

        private fun response(id: String, result: JsonObject): String = buildJsonObject {
            put("v", 1)
            put("kind", "res")
            put("id", id)
            put("ok", true)
            put("result", result)
        }.toString()

        private fun errorResponse(id: String): String = buildJsonObject {
            put("v", 1)
            put("kind", "res")
            put("id", id)
            put("ok", false)
            put("error", buildJsonObject {
                put("code", "internal")
                put("message", "test failure")
            })
        }.toString()
    }

    private class MemoryPersistentStore : PersistentStore {
        private val values = mutableMapOf<String, String>()
        override suspend fun read(key: String): String? = values[key]
        override suspend fun write(key: String, value: String) { values[key] = value }
        override suspend fun delete(key: String) { values.remove(key) }
    }

    private class MemoryCredentialStore(initial: Map<String, String>) : CredentialStore {
        private val values = initial.toMutableMap()
        override suspend fun read(hostId: String): String? = values[hostId]
        override suspend fun write(hostId: String, password: String) { values[hostId] = password }
        override suspend fun delete(hostId: String) { values.remove(hostId) }
    }
}

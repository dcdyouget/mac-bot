package bot.mac.mobile.core.network

import io.ktor.client.HttpClient
import io.ktor.client.plugins.websocket.webSocket
import io.ktor.http.HttpHeaders
import io.ktor.websocket.Frame
import io.ktor.websocket.WebSocketSession
import io.ktor.websocket.close
import io.ktor.websocket.readBytes
import io.ktor.websocket.readText
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

data class ScreenTab(
    val tabId: String,
    val title: String,
    val url: String,
    val assignmentId: String?,
    val active: Boolean,
)

data class ScreenState(
    val botId: String,
    val driver: String,
    val tabs: List<ScreenTab>,
    val width: Int,
    val height: Int,
)

/** Independent /ws/screen stream. It can be opened and closed without affecting MainConnection. */
class ScreenConnection(
    private val client: HttpClient,
    private val host: HostProfile,
    private val botId: String,
    private val quality: String = "auto",
    private val tabId: String? = null,
    private val scope: CoroutineScope,
    private val onFrame: suspend (ScreenFrame) -> Unit = {},
    private val backoff: BackoffPolicy = BackoffPolicy(),
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val sessionLock = Mutex()
    private var socket: WebSocketSession? = null
    private var runner: Job? = null
    private var requestedTab: String? = tabId
    private val _state = kotlinx.coroutines.flow.MutableStateFlow<ScreenState?>(null)

    val state: kotlinx.coroutines.flow.StateFlow<ScreenState?> = _state

    fun start(): Job {
        if (runner?.isActive == true) return runner!!
        runner = scope.launch(Dispatchers.Default) { runLoop() }
        return runner!!
    }

    suspend fun stop() {
        // Close the socket first so an incoming-frame loop leaves webSocket{} before
        // its runner is cancelled. The close must survive caller cancellation.
        val current = sessionLock.withLock { socket.also { socket = null } }
        withContext(NonCancellable) {
            try {
                current?.close()
            } catch (_: Throwable) {
                // Stop remains best-effort if the peer already closed the stream.
            }
        }
        runner?.cancel()
        runner?.join()
        runner = null
    }

    suspend fun switchTab(tabId: String): Boolean {
        sessionLock.withLock { requestedTab = tabId }
        return send(ScreenFrameCodec.switchTab(tabId))
    }

    suspend fun sendInput(input: JsonObject): Boolean = send(buildJsonObject {
        put("type", "input")
        put("event", input)
    }.toString())

    private suspend fun send(text: String): Boolean {
        val current = sessionLock.withLock { socket } ?: return false
        return try {
            current.send(Frame.Text(text))
            true
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            false
        }
    }

    private suspend fun runLoop() {
        var attempt = 0
        var addressIndex = 0
        while (scope.isActive && runner?.isActive != false) {
            if (host.addresses.isEmpty()) {
                delay(backoff.delayMillis(attempt++))
                continue
            }
            try {
                connect(host.addresses[addressIndex])
                attempt = 0
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                attempt++
            } finally {
                sessionLock.withLock { socket = null }
            }
            addressIndex = nextAddressIndex(host.addresses.size, addressIndex)
            if (scope.isActive && runner?.isActive != false) delay(backoff.delayMillis(attempt))
        }
    }

    private suspend fun connect(address: String) {
        val endpoint = EndpointBuilder.screen(address, botId, quality,
            sessionLock.withLock { requestedTab })
        client.webSocket(urlString = endpoint, request = {
            headers.append(HttpHeaders.Authorization, "Bearer ${host.password}")
        }) {
            sessionLock.withLock { socket = this }
            try {
                for (frame in incoming) {
                    when (frame) {
                        is Frame.Text -> parseState(frame.readText())
                        is Frame.Binary -> {
                            val screenFrame = ScreenFrameCodec.decode(frame.readBytes())
                            onFrame(screenFrame)
                            // Ack only after the render callback has consumed this frame.
                            check(send(ScreenFrameCodec.ack(screenFrame.header.seq))) {
                                "screen frame ack failed"
                            }
                        }
                        else -> Unit
                    }
                }
            } finally {
                sessionLock.withLock { socket = null }
            }
        }
    }

    private fun parseState(text: String) {
        val root = runCatching { json.parseToJsonElement(text) as? JsonObject }.getOrNull() ?: return
        if (root.string("type") != "state") return
        val value = root.objectValue("state") ?: return
        val tabs = (value["tabs"] as? kotlinx.serialization.json.JsonArray)?.mapNotNull { raw ->
            val tab = raw as? JsonObject ?: return@mapNotNull null
            ScreenTab(
                tab.string("tab_id") ?: return@mapNotNull null,
                tab.string("title") ?: "",
                tab.string("url") ?: "",
                tab.string("assignment_id"),
                tab["active"]?.toString()?.toBoolean() ?: false,
            )
        } ?: emptyList()
        _state.value = ScreenState(
            botId = value.string("bot_id") ?: botId,
            driver = value.string("driver") ?: "idle",
            tabs = tabs,
            width = value.long("width")?.toInt() ?: 0,
            height = value.long("height")?.toInt() ?: 0,
        )
    }
}

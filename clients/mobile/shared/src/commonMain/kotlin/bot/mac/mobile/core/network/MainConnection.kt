package bot.mac.mobile.core.network

import io.ktor.client.HttpClient
import io.ktor.client.plugins.ResponseException
import io.ktor.client.plugins.websocket.webSocket
import io.ktor.http.HttpHeaders
import io.ktor.websocket.Frame
import io.ktor.websocket.WebSocketSession
import io.ktor.websocket.close
import io.ktor.websocket.readText
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

private const val REQUEST_TIMEOUT_MILLIS = 30_000L
private const val HEARTBEAT_MILLIS = 20_000L

/**
 * Redacted connection diagnostics for field diagnosis.  Keep this type limited
 * to protocol stages and transport metadata; it must never carry an endpoint,
 * header, credential, or exception message.
 */
data class ConnectionDiagnostic(
    val stage: String,
    val httpStatus: Int? = null,
    val exceptionClass: String? = null,
)

private class SessionEvents {
    val mutex = Mutex()
    var buffering = true
    val buffered = mutableListOf<MainEvent>()
    val bufferedPersistentSeqs = mutableSetOf<Long>()
}

/**
 * The long-lived /ws connection. It owns protocol handshaking and reconnects;
 * UI code only observes state/events and calls [request].
 */
class MainConnection(
    private val client: HttpClient,
    private val host: HostProfile,
    private val identity: ClientIdentity,
    private val lastSeqStore: LastSeqStore,
    private val scope: CoroutineScope,
    private val backoff: BackoffPolicy = BackoffPolicy(),
    private val ids: IdGenerator = RandomIdGenerator,
    private val onEvent: suspend (MainEvent) -> Unit = {},
    private val heartbeatMillis: Long = HEARTBEAT_MILLIS,
    private val requestTimeoutMillis: Long = REQUEST_TIMEOUT_MILLIS,
    private val onDiagnostic: (ConnectionDiagnostic) -> Unit = { diagnostic ->
        println(
            "MainConnection diagnostic stage=${diagnostic.stage}" +
                " http_status=${diagnostic.httpStatus ?: "none"}" +
                " exception_class=${diagnostic.exceptionClass ?: "none"}",
        )
    },
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val lock = Mutex()
    private val requests = RequestTracker(ids)
    private val _snapshot = MutableStateFlow(ConnectionSnapshot(ConnectionStatus.STOPPED, lastSeq = host.lastSeq))
    private val _status = MutableStateFlow(ConnectionStatus.STOPPED)
    private val _events = MutableSharedFlow<MainEvent>(extraBufferCapacity = 128)
    private val _hello = MutableSharedFlow<JsonObject>(replay = 1, extraBufferCapacity = 1)
    private val session = MutableStateFlow<WebSocketSession?>(null)
    private val ready = MutableStateFlow(false)
    private var observedNodeId: String? = null
    private var cursor = host.lastSeq
    private var cursorInitialized = false
    private var activeSessionToken: Any? = null
    private var runner: Job? = null

    val snapshot: StateFlow<ConnectionSnapshot> = _snapshot.asStateFlow()
    val status: StateFlow<ConnectionStatus> = _status.asStateFlow()
    val events: SharedFlow<MainEvent> = _events.asSharedFlow()

    fun start(): Job {
        if (runner?.isActive == true) return runner!!
        runner = scope.launch(Dispatchers.Default) { runLoop() }
        return runner!!
    }

    suspend fun stop() {
        val current = lock.withLock {
            requests.cancelAll(NetworkError("connection_stopped", "connection was stopped"))
            session.value
        }
        // Closing the socket first lets the webSocket{} block leave its receive loop
        // before the runner is cancelled. Keep this cleanup alive if the caller is
        // itself being cancelled (for example when a host is deleted).
        withContext(NonCancellable) {
            try {
                current?.close()
            } catch (_: Throwable) {
                // The peer may already have closed the connection.
            }
        }
        runner?.cancel()
        runner?.join()
        runner = null
        session.value = null
        ready.value = false
        setSnapshot(_snapshot.value.copy(status = ConnectionStatus.STOPPED))
    }

    suspend fun close() = stop()

    /** Convenience overload for repositories that retain the host separately. */
    fun start(host: HostConnection): Job {
        require(host.id == this.host.id) { "connection belongs to host ${this.host.id}" }
        return start()
    }

    /** Executes a protocol method. Write methods receive a stable id for retry after reconnect. */
    suspend fun request(
        method: String,
        params: JsonObject = buildJsonObject { },
        write: Boolean = false,
        clientRequestId: String? = if (write) ids.nextId() else null,
    ): JsonObject = requestInternal(method, params, write, clientRequestId, requireReady = true)

    private suspend fun requestInternal(
        method: String,
        params: JsonObject,
        write: Boolean,
        clientRequestId: String?,
        requireReady: Boolean,
    ): JsonObject {
        val tracked = lock.withLock { requests.register(method, params, write, clientRequestId) }
        try {
            return withTimeout(requestTimeoutMillis) {
                if (requireReady) ready.first { it }
                val current = session.first { it != null }
                val sessionToken = lock.withLock { activeSessionToken }
                // Keep the pending entry if the socket closes while writing. The reconnect
                // handshake will replay it with the same request/client_request_id.
                try {
                    if (sessionToken != null) sendPending(current!!, tracked, sessionToken)
                } catch (_: Throwable) {
                    // The pending request is replayed by the next successful handshake.
                }
                tracked.result.await()
            }
        } finally {
            // A request that timed out must not be replayed forever. Reconnect replay
            // happens while the caller is still waiting; completed requests are removed here.
            withContext(NonCancellable) { lock.withLock { requests.remove(tracked.id) } }
        }
    }

    private suspend fun runLoop() {
        var attempt = 0
        var addressIndex = 0
        while (scope.isActive && runner?.isActive != false) {
            if (host.addresses.isEmpty()) {
                setSnapshot(_snapshot.value.copy(status = ConnectionStatus.RECONNECTING,
                    error = IllegalArgumentException("Host has no addresses"))
                )
                delay(backoff.delayMillis(attempt++))
                continue
            }
            val address = host.addresses[addressIndex]
            setSnapshot(_snapshot.value.copy(
                status = if (attempt == 0) ConnectionStatus.CONNECTING else ConnectionStatus.RECONNECTING,
                address = address,
                error = null,
            ))
            try {
                ready.value = false
                connect(address)
                attempt = 0
            } catch (cancelled: CancellationException) {
                if (cancelled is TimeoutCancellationException && scope.isActive && runner?.isActive != false) {
                    emitDiagnostic("reconnect", cancelled)
                    setSnapshot(_snapshot.value.copy(status = ConnectionStatus.RECONNECTING, error = cancelled))
                    attempt++
                } else {
                    throw cancelled
                }
            } catch (error: Throwable) {
                emitDiagnostic("reconnect", error)
                setSnapshot(_snapshot.value.copy(status = ConnectionStatus.RECONNECTING, error = error))
                attempt++
            } finally {
                session.value = null
            }
            addressIndex = nextAddressIndex(host.addresses.size, addressIndex)
            if (scope.isActive && runner?.isActive != false) delay(backoff.delayMillis(attempt))
        }
    }

    private suspend fun connect(address: String) {
        val endpoint = EndpointBuilder.main(address)
        var stage = "websocket_open"
        try {
            emitDiagnostic(stage)
            client.webSocket(urlString = endpoint, request = {
                headers.append(HttpHeaders.Authorization, "Bearer ${host.password}")
            }) {
                val currentSession = this
                session.value = this
                val sessionToken = Any()
                lock.withLock { activeSessionToken = sessionToken }
                val hello = CompletableDeferred<JsonObject>()
                val syncDone = CompletableDeferred<Long>()
                val sessionEvents = SessionEvents()
                val receiver = launch {
                    for (frame in incoming) processFrame(frame, hello, syncDone, sessionEvents)
                }
                val heartbeat = launch {
                    while (isActive) {
                        delay(heartbeatMillis)
                        // OkHttp only accepts data/close frames through Ktor's send();
                        // use the protocol heartbeat on platforms without manual ping.
                        requestInternal("ping", buildJsonObject {}, false, null, requireReady = true)
                    }
                }
                try {
                    stage = "hello"
                    emitDiagnostic(stage)
                    val helloData = withTimeout(requestTimeoutMillis) { hello.await() }
                    val protocol = helloData.long("protocol")
                    if (protocol !in 1L..3L) {
                        throw NetworkError("version_unsupported", "unsupported protocol version: $protocol")
                    }
                    val nodeId = helloData.string("node_id")
                        ?: throw NetworkError("internal", "hello is missing node_id")
                    val knownNodeId = observedNodeId
                    if (knownNodeId != null && knownNodeId != nodeId) {
                        throw NetworkError("conflict", "host address belongs to another node")
                    }
                    observedNodeId = nodeId
                    _hello.tryEmit(helloData)
                    emitEvent(MainEvent.Hello(helloData))
                    stage = "session_resume"
                    emitDiagnostic(stage)
                    val lastSeq = currentCursor()
                    val resume = requestInternal("session.resume", buildJsonObject {
                        put("last_seq", lastSeq)
                        put("client", buildJsonObject {
                            put("platform", identity.platform)
                            put("app_version", identity.appVersion)
                            put("device_name", identity.deviceName)
                            put("device_id", identity.deviceId)
                        })
                    }, write = false, clientRequestId = null, requireReady = false)
                    if (resume.string("mode") == "reset") {
                        stage = "bootstrap"
                        emitDiagnostic(stage)
                        val bootstrap = requestInternal("bootstrap", buildJsonObject { }, false, null, requireReady = false)
                        val bootstrapSeq = bootstrap.long("seq")
                        // Bootstrap is delivered to consumers as a synthetic event so state stores
                        // can replace their snapshot without depending on protocol model classes.
                        emitEvent(MainEvent.Ephemeral("bootstrap", bootstrap))
                        if (bootstrapSeq != null) resetSeq(bootstrapSeq)
                        flushBuffered(sessionEvents, bootstrapSeq ?: cursor)
                    } else {
                        // replay events arrive before sync.done; wait for the cursor barrier.
                        stage = "sync_done"
                        emitDiagnostic(stage)
                        withTimeout(requestTimeoutMillis) { syncDone.await() }
                        flushBuffered(sessionEvents, cursor)
                    }
                    stage = "ready"
                    emitDiagnostic(stage)
                    resendPending(this, sessionToken)
                    ready.value = true
                    setSnapshot(_snapshot.value.copy(
                        status = ConnectionStatus.CONNECTED,
                        lastSeq = cursor, error = null,
                    ))
                    receiver.join()
                } finally {
                    ready.value = false
                    lock.withLock { if (activeSessionToken === sessionToken) activeSessionToken = null }
                    heartbeat.cancel()
                    receiver.cancel()
                    heartbeat.join()
                    receiver.join()
                    currentSession.close()
                }
            }
        } catch (error: Throwable) {
            emitDiagnostic(stage, error)
            throw error
        }
    }

    private fun emitDiagnostic(stage: String, error: Throwable? = null) {
        var cause = error
        var status: Int? = null
        repeat(4) {
            if (status == null && cause != null) {
                status = (cause as? ResponseException)?.response?.status?.value
                cause = cause.cause
            }
        }
        onDiagnostic(
            ConnectionDiagnostic(
                stage = stage,
                httpStatus = status,
                exceptionClass = error?.let { it::class.simpleName ?: "Unknown" },
            ),
        )
    }

    private suspend fun processFrame(
        frame: Frame,
        hello: CompletableDeferred<JsonObject>,
        syncDone: CompletableDeferred<Long>,
        sessionEvents: SessionEvents,
    ) {
        val obj = when (frame) {
            is Frame.Text -> runCatching { json.parseToJsonElement(frame.readText()) as? JsonObject }.getOrNull()
            else -> null
        } ?: return
        when (obj.string("kind")) {
            "res" -> {
                val id = obj.string("id") ?: return
                val waiter = lock.withLock { requests.get(id)?.result } ?: return
                if (obj["ok"]?.jsonPrimitive?.content == "true") {
                    waiter.complete(obj.objectValue("result") ?: buildJsonObject { })
                } else {
                    val error = obj.objectValue("error")
                    waiter.completeExceptionally(NetworkError(
                        error?.string("code") ?: "internal",
                        error?.string("message") ?: "request failed",
                        error,
                    ))
                }
            }
            "evt" -> {
                val event = obj.string("event") ?: return
                val data = obj.objectValue("data") ?: buildJsonObject { }
                when (event) {
                    "hello" -> hello.complete(data)
                    "sync.done" -> syncDone.complete(obj.long("seq") ?: data.long("seq") ?: 0L)
                }
                if (event == "hello" || event == "sync.done") return
                val seq = obj.long("seq")
                dispatchEvent(sessionEvents, if (seq != null) {
                    MainEvent.Persistent(seq, event, data)
                } else {
                    MainEvent.Ephemeral(event, data)
                })
            }
        }
    }

    private suspend fun resendPending(ws: WebSocketSession, sessionToken: Any) {
        val pending = lock.withLock { requests.replayable() }
        for (request in pending) sendPending(ws, request, sessionToken)
    }

    private suspend fun sendPending(ws: WebSocketSession, request: TrackedRequest, sessionToken: Any) {
        val shouldSend = lock.withLock { requests.markSent(request, sessionToken) }
        if (!shouldSend) return
        ws.send(Frame.Text(request.frame.toString()))
    }

    private suspend fun saveSeq(seq: Long) {
        cursor = maxOf(cursor, seq)
        lastSeqStore.put(host.id, seq)
        setSnapshot(_snapshot.value.copy(lastSeq = cursor))
    }

    private suspend fun resetSeq(seq: Long) {
        cursorInitialized = true
        cursor = seq
        lastSeqStore.reset(host.id, seq)
        setSnapshot(_snapshot.value.copy(lastSeq = seq))
    }

    private suspend fun currentCursor(): Long {
        if (!cursorInitialized) {
            cursor = maxOf(cursor, lastSeqStore.get(host.id))
            cursorInitialized = true
        }
        return cursor
    }

    private suspend fun dispatchEvent(sessionEvents: SessionEvents, event: MainEvent) {
        sessionEvents.mutex.withLock {
            val seq = (event as? MainEvent.Persistent)?.seq
            if (sessionEvents.buffering) {
                if (seq == null || sessionEvents.bufferedPersistentSeqs.add(seq)) {
                    sessionEvents.buffered += event
                }
                return@withLock
            }
            if (seq != null && seq <= currentCursor()) return@withLock
            emitEvent(event)
            if (seq != null) saveSeq(seq)
        }
    }

    private suspend fun flushBuffered(sessionEvents: SessionEvents, dropThrough: Long) {
        sessionEvents.mutex.withLock {
            val buffered = sessionEvents.buffered.toList()
                .sortedWith(compareBy<MainEvent> { (it as? MainEvent.Persistent)?.seq ?: Long.MAX_VALUE })
            sessionEvents.buffered.clear()
            sessionEvents.bufferedPersistentSeqs.clear()
            for (event in buffered) {
                val seq = (event as? MainEvent.Persistent)?.seq
                if (seq != null && (seq <= dropThrough || seq <= currentCursor())) continue
                emitEvent(event)
                if (seq != null) saveSeq(seq)
            }
            sessionEvents.buffering = false
        }
    }

    private suspend fun emitEvent(event: MainEvent) {
        _events.emit(event)
        onEvent(event)
    }

    private fun setSnapshot(value: ConnectionSnapshot) {
        _snapshot.value = value
        _status.value = value.status
    }
}

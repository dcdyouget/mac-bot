package bot.mac.mobile.core.state

import bot.mac.mobile.core.network.ClientIdentity
import bot.mac.mobile.core.network.ConnectionSnapshot
import bot.mac.mobile.core.network.ConnectionStatus
import bot.mac.mobile.core.network.EndpointBuilder
import bot.mac.mobile.core.network.HostProfile
import bot.mac.mobile.core.network.LastSeqStore
import bot.mac.mobile.core.network.MainConnection
import bot.mac.mobile.core.network.MainEvent
import bot.mac.mobile.core.network.RandomIdGenerator
import bot.mac.mobile.core.network.ScreenConnection
import bot.mac.mobile.core.network.ScreenFrame
import bot.mac.mobile.core.platform.*
import bot.mac.mobile.core.protocol.*
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.websocket.WebSocketDeflateExtension
import io.ktor.client.plugins.websocket.WebSockets
import io.ktor.client.request.*
import io.ktor.client.request.forms.*
import io.ktor.client.statement.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.*

@Serializable
data class SavedHost(val id: String, val name: String, val addresses: List<String>, val nodeId: String? = null)

private class HostSession(
    val id: String,
    val generation: Long,
    val profile: HostProfile,
    val connection: MainConnection,
) {
    var monitor: Job? = null
    var refresh: Job? = null
}

class ClientRepository(
    private val storage: PersistentStore = platformPersistentStore(),
    private val credentials: CredentialStore = platformCredentialStore(),
) : MobileRepository {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val mutex = Mutex()
    private val hostMutex = Mutex()
    private val initMutex = Mutex()
    private var initialized = false
    private val hostGeneration = mutableMapOf<String, Long>()
    private val hostSessions = mutableMapOf<String, HostSession>()
    private val writeMutex = Mutex()
    /**
     * A write id is a retry lease, rather than an id for the request's payload.
     * Multiple identical user actions may be in flight at the same time, so each
     * active call needs a distinct id. Failed leases remain in this queue and are
     * reused by a later retry of the same action.
     */
    private val pendingWrites = mutableMapOf<String, MutableList<String>>()
    private val activeWriteIds = mutableMapOf<String, MutableSet<String>>()
    private val client = HttpClient {
        install(WebSockets) { extensions { install(WebSocketDeflateExtension) } }
    }

    private val _state = MutableStateFlow(MobileState())
    override val state: StateFlow<MobileState> = _state.asStateFlow()
    private val _hostStates = MutableStateFlow<Map<String, MobileState>>(emptyMap())
    val hostStates: StateFlow<Map<String, MobileState>> = _hostStates.asStateFlow()
    private val _hosts = MutableStateFlow<List<SavedHost>>(emptyList())
    val hosts: StateFlow<List<SavedHost>> = _hosts.asStateFlow()
    private val _activeHost = MutableStateFlow<HostProfile?>(null)
    override val activeHost: StateFlow<HostProfile?> = _activeHost.asStateFlow()
    private val _hostStatuses = MutableStateFlow<Map<String, ConnectionSnapshot>>(emptyMap())
    override val hostStatuses: StateFlow<Map<String, ConnectionSnapshot>> = _hostStatuses.asStateFlow()
    private val _connection = MutableStateFlow(ConnectionSnapshot(ConnectionStatus.STOPPED))
    val connectionStatus: StateFlow<ConnectionSnapshot> = _connection.asStateFlow()
    private val _hostEvents = MutableSharedFlow<HostEvent>(extraBufferCapacity = 128)
    override val hostEvents: SharedFlow<HostEvent> = _hostEvents.asSharedFlow()
    private val _events = MutableSharedFlow<MainEvent>(extraBufferCapacity = 64)
    val events: SharedFlow<MainEvent> = _events.asSharedFlow()

    val theme = MutableStateFlow("system")
    val notifications = MutableStateFlow(true)
    val deepLink = MutableStateFlow<String?>(null)

    suspend fun initialize() = initMutex.withLock {
        if (initialized) return@withLock
        _hosts.value = storage.hostRecordsJson()
            ?.let { runCatching { protocolJson.decodeFromString<List<SavedHost>>(it) }.getOrNull() }
            .orEmpty()
        theme.value = storage.read("theme") ?: "system"
        notifications.value = storage.read("notifications") != "false"
        _hostStates.value = _hosts.value.associate { it.id to loadState(it.id) }
        _hostStatuses.value = _hosts.value.associate { it.id to ConnectionSnapshot(ConnectionStatus.STOPPED) }
        initialized = true
        _hosts.value.forEach { startHost(it) }
        (storage.read("selected_host") ?: _hosts.value.firstOrNull()?.id)?.let { selectHost(it) }
    }

    suspend fun background(): Nothing { initialize(); awaitCancellation() }
    suspend fun setTheme(value: String) { theme.value = value; storage.write("theme", value) }
    suspend fun setNotifications(value: Boolean) { notifications.value = value; storage.write("notifications", value.toString()) }

    suspend fun saveHost(name: String, addresses: List<String>, password: String, existingId: String? = null) {
        require(name.isNotBlank() && addresses.isNotEmpty() && password.isNotEmpty())
        addresses.forEach { EndpointBuilder.main(it) }
        val id = existingId ?: RandomIdGenerator.nextId()
        credentials.write(id, password)
        val saved = SavedHost(id, name.trim(), addresses)
        _hosts.value = _hosts.value.filterNot { it.id == id } + saved
        storage.setHostRecordsJson(protocolJson.encodeToString(_hosts.value))
        if (initialized) startHost(saved)
        selectHost(id)
    }

    suspend fun deleteHost(id: String) {
        val removed = hostMutex.withLock {
            val old = hostSessions.remove(id)
            hostGeneration[id] = (hostGeneration[id] ?: 0L) + 1L
            _hostStates.update { it - id }
            _hostStatuses.update { it - id }
            old
        }
        removed?.let { stopSession(it) }
        credentials.delete(id)
        storage.delete("snapshot:" + id)
        storage.delete("last_seq:" + id)
        _hosts.value = _hosts.value.filterNot { it.id == id }
        storage.setHostRecordsJson(protocolJson.encodeToString(_hosts.value))
        if (_activeHost.value?.id == id) {
            clearActive()
            _hosts.value.firstOrNull()?.let { selectHost(it.id) }
        }
    }

    suspend fun clearActive() {
        val old = hostMutex.withLock {
            val id = _activeHost.value?.id
            val session = id?.let { hostSessions.remove(it) }
            if (id != null) {
                hostGeneration[id] = (hostGeneration[id] ?: 0L) + 1L
                _hostStates.value = _hostStates.value - id
                _hostStatuses.value = _hostStatuses.value - id
            }
            _activeHost.value = null
            _state.value = MobileState()
            _connection.value = ConnectionSnapshot(ConnectionStatus.STOPPED)
            session
        }
        old?.let { stopSession(it) }
        storage.delete("selected_host")
    }

    suspend fun clearActiveHost() = clearActive()

    suspend fun selectHost(id: String) {
        val saved = _hosts.value.firstOrNull { it.id == id } ?: return
        val password = credentials.read(id).orEmpty()
        val needsStart = hostMutex.withLock { initialized && password.isNotBlank() && hostSessions[id] == null }
        if (needsStart) startHost(saved)
        val cached = _hostStates.value[id] ?: loadState(id)
        val status = _hostStatuses.value[id] ?: ConnectionSnapshot(ConnectionStatus.STOPPED)
        val profile = HostProfile(id, saved.name, saved.addresses, password, cached.lastSeq)
        hostMutex.withLock {
            _activeHost.value = profile
            _state.value = cached.copy(
                connected = status.status == ConnectionStatus.CONNECTED,
                error = status.error?.message ?: cached.error,
            )
            _connection.value = status
        }
        storage.write("selected_host", id)
    }

    override suspend fun call(method: String, params: JsonObject): JsonObject {
        val id = hostMutex.withLock { _activeHost.value?.id } ?: error("Host is not selected")
        return callOnHost(id, method, params)
    }

    override suspend fun callOnHost(hostId: String, method: String, params: JsonObject): JsonObject {
        val session = hostMutex.withLock { hostSessions[hostId] ?: error("Host is not connected: " + hostId) }
        val write = method in WRITE_METHODS
        val retryKey = hostId + ":" + method + ":" + params
        val stableId = if (write) leaseWriteId(retryKey) else null
        try {
            val result = session.connection.request(method, params, write = write, clientRequestId = stableId)
            if (write) completeWrite(retryKey, stableId!!)
            hostMutex.withLock {
                if (hostSessions[hostId] !== session) return@withLock
                mutex.withLock {
                    val next = RpcStateReducer.apply(_hostStates.value[hostId] ?: MobileState(), method, params, result).copy(error = null)
                    persist(hostId, next)
                    _hostStates.value = _hostStates.value + (hostId to next)
                    publishActiveState(hostId, next)
                }
            }
            return result
        } catch (cancelled: CancellationException) {
            if (write) withContext(NonCancellable) { releaseWrite(retryKey, stableId!!) }
            throw cancelled
        } catch (failure: Throwable) {
            if (write) releaseWrite(retryKey, stableId!!)
            hostMutex.withLock {
                if (hostSessions[hostId] === session) {
                    mutex.withLock {
                        val next = (_hostStates.value[hostId] ?: MobileState()).copy(error = failure.message)
                        _hostStates.value = _hostStates.value + (hostId to next)
                        publishActiveState(hostId, next)
                    }
                }
            }
            throw failure
        }
    }

    private suspend fun leaseWriteId(retryKey: String): String = writeMutex.withLock {
        val candidates = pendingWrites[retryKey] ?: decodeWriteIds(storage.read(writeIdKey(retryKey))).also {
            pendingWrites[retryKey] = it
        }
        val active = activeWriteIds.getOrPut(retryKey) { mutableSetOf() }
        val id = candidates.firstOrNull { it !in active } ?: RandomIdGenerator.nextId().also { candidates += it }
        active += id
        persistWriteIds(retryKey, candidates)
        id
    }

    private suspend fun completeWrite(retryKey: String, id: String) = writeMutex.withLock {
        activeWriteIds[retryKey]?.let { active ->
            active.remove(id)
            if (active.isEmpty()) activeWriteIds.remove(retryKey)
        }
        pendingWrites[retryKey]?.let { candidates ->
            candidates.remove(id)
            if (candidates.isEmpty()) pendingWrites.remove(retryKey)
            persistWriteIds(retryKey, candidates)
        }
    }

    /** Releases the active lease while retaining the id for a later user retry. */
    private suspend fun releaseWrite(retryKey: String, id: String) = writeMutex.withLock {
        activeWriteIds[retryKey]?.let { active ->
            active.remove(id)
            if (active.isEmpty()) activeWriteIds.remove(retryKey)
        }
        pendingWrites[retryKey]?.let { persistWriteIds(retryKey, it) }
    }

    private fun writeIdKey(retryKey: String): String = "write_id:" + retryKey

    private fun decodeWriteIds(raw: String?): MutableList<String> {
        val value = raw?.trim().orEmpty()
        if (value.isBlank()) return mutableListOf()
        if (!value.startsWith("[")) return mutableListOf(value)
        return runCatching {
            protocolJson.parseToJsonElement(value).jsonArray
                .mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                .toMutableList()
        }.getOrDefault(mutableListOf())
    }

    private suspend fun persistWriteIds(retryKey: String, ids: List<String>) {
        if (ids.isEmpty()) storage.delete(writeIdKey(retryKey))
        else storage.write(writeIdKey(retryKey), JsonArray(ids.map(::JsonPrimitive)).toString())
    }

    override suspend fun refresh() {
        val id = hostMutex.withLock { _activeHost.value?.id } ?: error("Host is not selected")
        callOnHost(id, "chat.list")
        callOnHost(id, "bot.list")
        callOnHost(id, "project.list")
        callOnHost(id, "workbench.get")
    }

    override fun createScreen(botId: String, quality: String, tabId: String?, onFrame: suspend (ScreenFrame) -> Unit): ScreenConnection {
        val profile = _activeHost.value ?: error("Host is not selected")
        return ScreenConnection(client, profile, botId, quality, tabId, scope, onFrame)
    }

    override suspend fun uploadFile(file: PickedFile): JsonObject {
        require(file.bytes.size <= 100 * 1024 * 1024) { "File exceeds 100 MB" }
        val session = activeSession()
        val endpoint = EndpointBuilder.http(currentAddress(session), "/api/v1/uploads")
        val response = client.submitFormWithBinaryData(endpoint, formData {
            append("file", file.bytes, Headers.build {
                append(HttpHeaders.ContentType, file.mimeType ?: "application/octet-stream")
                append(HttpHeaders.ContentDisposition, "filename=\"" + file.name.replace("\"", "").replace("\r", "").replace("\n", "") + "\"")
            })
        }) { header(HttpHeaders.Authorization, "Bearer " + session.profile.password) }
        check(response.status.isSuccess()) { "Upload failed: " + response.status.value }
        return protocolJson.parseToJsonElement(response.bodyAsText()).jsonObject
    }

    override suspend fun fetchText(path: String, params: Map<String, String>): String = fetchBytes(path, params).decodeToString()

    override suspend fun fetchBytes(path: String, params: Map<String, String>): ByteArray {
        val session = activeSession()
        val response = client.get(EndpointBuilder.http(currentAddress(session), path)) {
            header(HttpHeaders.Authorization, "Bearer " + session.profile.password)
            params.forEach { (key, value) -> parameter(key, value) }
        }
        check(response.status.isSuccess()) { "Download failed: " + response.status.value }
        return response.body<ByteArray>()
    }

    private suspend fun activeSession(): HostSession = hostMutex.withLock {
        val id = _activeHost.value?.id ?: error("Host is not selected")
        hostSessions[id] ?: error("Host is not connected: " + id)
    }

    private fun currentAddress(session: HostSession): String =
        _hostStatuses.value[session.id]?.address ?: session.profile.addresses.first()

    private suspend fun startHost(saved: SavedHost) {
        val id = saved.id
        val old = hostMutex.withLock {
            val previous = hostSessions.remove(id)
            hostGeneration[id] = (hostGeneration[id] ?: 0L) + 1L
            previous
        }
        old?.let { stopSession(it) }
        val password = credentials.read(id).orEmpty()
        val cached = loadState(id)
        val generation = hostMutex.withLock { hostGeneration[id] ?: 0L }
        if (password.isBlank()) {
            hostMutex.withLock {
                if (hostGeneration[id] == generation) {
                    _hostStates.value = _hostStates.value + (id to cached)
                    _hostStatuses.value = _hostStatuses.value + (id to ConnectionSnapshot(ConnectionStatus.STOPPED, error = IllegalStateException("Host credential is unavailable")))
                }
            }
            return
        }
        lateinit var session: HostSession
        val profile = HostProfile(id, saved.name, saved.addresses, password, cached.lastSeq)
        val connection = MainConnection(
            client, profile,
            ClientIdentity(appVersion = "0.1.0", deviceName = "Mac Bot Android", deviceId = deviceId()),
            cursorStore(), scope,
            onEvent = { event -> handleEvent(session, event) },
        )
        session = HostSession(id, generation, profile, connection)
        val installed = hostMutex.withLock {
            if (hostGeneration[id] != generation || _hosts.value.none { it.id == id }) false
            else {
                hostSessions[id] = session
                _hostStates.value = _hostStates.value + (id to cached)
                _hostStatuses.value = _hostStatuses.value + (id to ConnectionSnapshot(ConnectionStatus.STOPPED))
                true
            }
        }
        if (!installed) return
        session.monitor = scope.launch {
            connection.snapshot.collect { snapshot ->
                hostMutex.withLock {
                    if (hostSessions[id] !== session) return@withLock
                    _hostStatuses.value = _hostStatuses.value + (id to snapshot)
                    if (_activeHost.value?.id == id) {
                        _connection.value = snapshot
                        _state.update { it.copy(connected = snapshot.status == ConnectionStatus.CONNECTED, error = snapshot.error?.message ?: it.error) }
                    }
                }
            }
        }
        session.refresh = scope.launch {
            connection.status.filter { it == ConnectionStatus.CONNECTED }.collect {
                listOf("chat.list", "bot.list", "project.list", "workbench.get", "skill.list", "routine.list", "provider.list").forEach { method -> runCatching { callOnHost(id, method, if(method == "bot.list") buildJsonObject { put("include_hidden",true) } else buildJsonObject {}) } }
            }
        }
        connection.start()
    }

    private suspend fun stopSession(session: HostSession) {
        runCatching { session.connection.stop() }
        session.monitor?.cancelAndJoin()
        session.refresh?.cancelAndJoin()
    }

    private suspend fun handleEvent(session: HostSession, event: MainEvent) {
        hostMutex.withLock {
            if (hostSessions[session.id] !== session) throw CancellationException("stale host session")
            mutex.withLock {
                val current = _hostStates.value[session.id] ?: MobileState()
                val next = when (event) {
                    is MainEvent.Hello -> {
                        val nodeId = event.data.str("node_id")
                        val saved = _hosts.value.firstOrNull { it.id == session.id }
                        require(saved?.nodeId == null || saved.nodeId == nodeId) { "Address belongs to a different Host" }
                        if (saved != null && saved.nodeId != nodeId) {
                            _hosts.value = _hosts.value.map { if (it.id == session.id) it.copy(nodeId = nodeId) else it }
                            storage.setHostRecordsJson(protocolJson.encodeToString(_hosts.value))
                        }
                        current.copy(hello = event.data)
                    }
                    is MainEvent.Persistent -> StateReducer.event(current, event.event, event.data, event.seq)
                    is MainEvent.Ephemeral -> StateReducer.event(current, event.event, event.data)
                }
                if (event is MainEvent.Persistent || event is MainEvent.Ephemeral && event.event == "bootstrap") persist(session.id, next)
                _hostStates.value = _hostStates.value + (session.id to next)
                publishActiveState(session.id, next)
                _hostEvents.emit(HostEvent(session.id, event, next))
                if (_activeHost.value?.id == session.id) _events.emit(event)
            }
        }
    }

    private fun publishActiveState(hostId: String, value: MobileState) {
        if (_activeHost.value?.id != hostId) return
        val status = _hostStatuses.value[hostId]
        _state.value = value.copy(connected = status?.status == ConnectionStatus.CONNECTED, error = value.error ?: status?.error?.message)
        _connection.value = status ?: ConnectionSnapshot(ConnectionStatus.STOPPED)
    }

    private fun cursorStore(): LastSeqStore = object : LastSeqStore {
        override suspend fun get(hostId: String): Long = minOf(storage.lastSeq(hostId), loadState(hostId).lastSeq)
        override suspend fun reset(hostId: String, seq: Long) { storage.write("last_seq:" + hostId, seq.toString()) }
        override suspend fun put(hostId: String, seq: Long) { storage.setLastSeq(hostId, seq) }
    }

    private suspend fun loadState(hostId: String): MobileState =
        storage.read("snapshot:" + hostId)?.let { runCatching { protocolJson.decodeFromString<MobileState>(it) }.getOrNull() } ?: MobileState()

    private suspend fun persist(hostId: String, value: MobileState) {
        withTimeout(5_000) {
            storage.write("snapshot:" + hostId, protocolJson.encodeToString(value.copy(connected = false, error = null, traces = emptyMap(), traceFragments = emptyMap(), typing = emptyMap())))
        }
    }

    private suspend fun deviceId(): String =
        storage.read("device_id") ?: RandomIdGenerator.nextId().also { storage.write("device_id", it) }


    companion object {
        val WRITE_METHODS = setOf("device.register", "chat.send", "chat.mark_read", "chat.react", "chat.set_pinned", "chat.set_muted", "bot.create", "bot.update", "bot.duplicate", "bot.delete", "bot.create_from_template", "project.create", "project.update", "project.add_member", "project.remove_member", "project.confirm_done", "project.request_changes", "project.archive", "project.reopen", "assignment.stop", "approval.decide", "question.answer", "loop.resolve", "takeover.start", "takeover.release", "skill.create", "skill.update", "skill.delete", "skill.set_enabled", "skill.publish", "skill.import", "routine.create", "routine.update", "routine.delete", "routine.set_enabled", "routine.test_run", "provider.create", "provider.update", "provider.delete", "provider.test", "model.refresh", "model.upsert", "model.delete", "settings.update")
    }
}

object AppRuntime { val repository by lazy { ClientRepository() } }

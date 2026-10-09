package bot.mac.mobile.core.network

import kotlinx.serialization.json.JsonObject

/** A host saved by the client. Addresses are tried in their listed order. */
data class HostProfile(
    val id: String,
    val name: String,
    val addresses: List<String>,
    val password: String,
    val lastSeq: Long = 0L,
) {
    constructor(id: String, addresses: List<String>, password: String, lastSeq: Long = 0L) :
        this(id, id, addresses, password, lastSeq)
}

/** Compatibility name used by the state/repository layer. */
typealias HostConnection = HostProfile

data class ClientIdentity(
    val platform: String = "android",
    val appVersion: String,
    val deviceName: String,
    val deviceId: String,
)

interface LastSeqStore {
    suspend fun get(hostId: String): Long
    suspend fun put(hostId: String, seq: Long)
    /** Reset is used after a server-issued bootstrap reset and may move the cursor backwards. */
    suspend fun reset(hostId: String, seq: Long) { put(hostId, seq) }
}

class InMemoryLastSeqStore : LastSeqStore {
    private val values = mutableMapOf<String, Long>()
    override suspend fun get(hostId: String): Long = values[hostId] ?: 0L
    override suspend fun put(hostId: String, seq: Long) {
        if (seq >= (values[hostId] ?: 0L)) values[hostId] = seq
    }
    override suspend fun reset(hostId: String, seq: Long) { values[hostId] = seq }
}

enum class ConnectionStatus { STOPPED, CONNECTING, CONNECTED, RECONNECTING }

sealed interface MainEvent {
    data class Hello(val data: JsonObject) : MainEvent
    data class Persistent(val seq: Long, val event: String, val data: JsonObject) : MainEvent
    data class Ephemeral(val event: String, val data: JsonObject) : MainEvent
}

data class ConnectionSnapshot(
    val status: ConnectionStatus,
    val address: String? = null,
    val lastSeq: Long = 0L,
    val error: Throwable? = null,
)

class NetworkError(
    val code: String,
    override val message: String,
    val details: JsonObject? = null,
) : IllegalStateException(message)

fun JsonObject.long(name: String): Long? = this[name]?.toString()?.trim('"')?.toLongOrNull()
fun JsonObject.string(name: String): String? = this[name]?.toString()?.trim('"')
fun JsonObject.objectValue(name: String): JsonObject? = this[name] as? JsonObject

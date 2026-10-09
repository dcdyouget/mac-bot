package bot.mac.mobile.core.network

import kotlinx.coroutines.CompletableDeferred
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * Pure request ledger used to make reconnect replay observable and testable.
 * A write keeps the same clientRequestId in [frame] for every replay.
 */
data class TrackedRequest(
    val id: String,
    val clientRequestId: String?,
    val frame: JsonObject,
    val result: CompletableDeferred<JsonObject> = CompletableDeferred(),
    // Identity of the WebSocket session on which this request was sent. This prevents a
    // request registered while the handshake is in progress from being sent twice when
    // the reconnect replay pass and its caller both reach the ready gate.
    var sentSession: Any? = null,
)

class RequestTracker(private val ids: IdGenerator = RandomIdGenerator) {
    private val pending = linkedMapOf<String, TrackedRequest>()

    fun register(
        method: String,
        params: JsonObject = buildJsonObject { },
        write: Boolean = false,
        clientRequestId: String? = if (write) ids.nextId() else null,
    ): TrackedRequest {
        val requestId = ids.nextId()
        val existingClientRequestId = params["client_request_id"]?.toString()?.trim('"')
        val stableClientRequestId = if (write) clientRequestId ?: existingClientRequestId ?: ids.nextId() else null
        val requestParams = if (write && stableClientRequestId != null && params["client_request_id"] == null) {
            buildJsonObject {
                params.forEach { (key, value) -> put(key, value) }
                put("client_request_id", stableClientRequestId)
            }
        } else params
        val tracked = TrackedRequest(
            id = requestId,
            clientRequestId = stableClientRequestId,
            frame = buildJsonObject {
                put("v", 1)
                put("kind", "req")
                put("id", requestId)
                put("method", method)
                put("params", requestParams)
            },
        )
        pending[requestId] = tracked
        return tracked
    }

    fun replayable(): List<TrackedRequest> = pending.values.filter { !it.result.isCompleted }
    fun markSent(request: TrackedRequest, session: Any): Boolean {
        if (request.sentSession === session) return false
        request.sentSession = session
        return true
    }
    fun get(id: String): TrackedRequest? = pending[id]
    fun remove(id: String) { pending.remove(id) }
    fun cancelAll(cause: Throwable) {
        pending.values.forEach { it.result.completeExceptionally(cause) }
        pending.clear()
    }
    fun clear() { pending.clear() }
    fun size(): Int = pending.size
}

package bot.mac.mobile

import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable

@Serializable
internal enum class NotificationKind { APPROVAL, NEEDS_YOU, COMPLETED, MESSAGE }

@Serializable
internal data class PendingNotification(
    val kind: NotificationKind,
    val hostId: String,
    val id: String,
    val title: String,
    val text: String,
    val seq: Long? = null,
    val projectId: String? = null,
    val chatId: String? = null,
    val deepLinkKind: String? = null,
    val deepLinkId: String? = null,
) {
    val ledgerKey: String get() = "$hostId:${when (kind) {
        NotificationKind.APPROVAL -> "approval"
        NotificationKind.NEEDS_YOU -> "needs"
        NotificationKind.COMPLETED -> "completed"
        NotificationKind.MESSAGE -> "message"
    }}:$id"
    val marker: String get() = seq?.toString() ?: "seen"
    val identity: NotificationIdentity get() {
        // Private messages share one tray entry per Host/chat. The ledger still
        // tracks individual messages, so replay cannot resurrect older content.
        val tag = if (kind == NotificationKind.MESSAGE && !chatId.isNullOrBlank())
            "macbot:$hostId:chat:$chatId" else "macbot:$ledgerKey"
        val hash = tag.hashCode()
        return NotificationIdentity(tag, if (hash == 100) 101 else hash)
    }
}

internal interface NotificationDeliveryStore {
    fun pending(): List<PendingNotification>
    fun savePending(value: List<PendingNotification>)
    fun marker(key: String): String?
    fun markShown(key: String, marker: String)
}

internal interface NotificationDeliveryBackend {
    fun allowed(): Boolean
    fun allowed(request: PendingNotification): Boolean = allowed()
    fun active(): List<RetainedNotification>
    fun prepare(request: PendingNotification) = Unit
    fun cancel(identity: NotificationIdentity)
    fun post(request: PendingNotification)
    fun shown(request: PendingNotification): Boolean
}

/** Serializes quota reservations and commits deduplication only after OS visibility. */
internal class NotificationDeliveryCoordinator(
    private val store: NotificationDeliveryStore,
    private val backend: NotificationDeliveryBackend,
    private val pause: suspend () -> Unit = { delay(150) },
) {
    private val mutex = Mutex()
    private var pending: LinkedHashMap<String, PendingNotification>? = null

    private fun queue(): LinkedHashMap<String, PendingNotification> = pending ?: linkedMapOf<String, PendingNotification>()
        .apply { store.pending().forEach { put(it.ledgerKey, it) } }.also { pending = it }

    private fun acknowledged(request: PendingNotification): Boolean {
        val marker = store.marker(request.ledgerKey)
        if (marker == request.marker) return true
        val acknowledgedSeq = marker?.toLongOrNull()
        return request.seq != null && acknowledgedSeq != null && request.seq <= acknowledgedSeq
    }

    suspend fun enqueue(request: PendingNotification) = mutex.withLock {
        if (acknowledged(request)) return@withLock
        val queue = queue()
        val previous = queue[request.ledgerKey]
        if (previous?.seq != null && request.seq != null && previous.seq > request.seq) return@withLock
        queue[request.ledgerKey] = request
        store.savePending(queue.values.toList())
    }

    /** Failed/silently rejected deliveries remain durable for the next attempt/startup. */
    suspend fun deliverPending(): Boolean = mutex.withLock {
        val queue = queue()
        for (request in queue.values.toList()) {
            if (!backend.allowed()) break
            if (!backend.allowed(request)) continue
            if (acknowledged(request)) {
                queue.remove(request.ledgerKey)
                store.savePending(queue.values.toList())
                continue
            }
            var delivered = false
            var posted = false
            repeat(4) {
                if (delivered || !backend.allowed(request)) return@repeat
                if (backend.shown(request)) {
                    delivered = true
                    return@repeat
                }
                if (posted) {
                    pause()
                    delivered = backend.shown(request)
                    return@repeat
                }
                // Resolve resources and PendingIntents before reclaiming old tray
                // entries, so a malformed payload cannot evict useful entries.
                backend.prepare(request)
                val active = backend.active()
                NotificationRetention.evictions(active, request.identity).forEach(backend::cancel)
                val remaining = backend.active()
                if (remaining.size + (if (remaining.any { it.identity == request.identity }) 0 else 1) > NotificationRetention.MAX_ACTIVE) {
                    pause()
                    return@repeat
                }
                backend.post(request)
                posted = true
                pause()
                // Check payload identity as well as tag/id: an existing coalesced
                // chat notification is not proof that this update was accepted.
                delivered = backend.shown(request)
            }
            if (delivered) {
                store.markShown(request.ledgerKey, request.marker)
                queue.remove(request.ledgerKey)
                store.savePending(queue.values.toList())
            }
        }
        queue.isNotEmpty()
    }
}

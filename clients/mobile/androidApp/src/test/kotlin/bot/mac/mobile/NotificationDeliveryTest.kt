package bot.mac.mobile

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class NotificationDeliveryTest {
    @Test
    fun fullFiftyEntryStoreDeliversNeedsCompletedAndMessageWithoutSilentDrop() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend(seedCount = 50)
        val coordinator = coordinator(store, backend)
        val requests = listOf(
            request(NotificationKind.NEEDS_YOU, "needs-1", seq = 11),
            request(NotificationKind.COMPLETED, "completed-1", seq = 12, projectId = "project-1"),
            request(NotificationKind.MESSAGE, "message-1", seq = 13, chatId = "chat-1"),
        )

        requests.forEach { coordinator.enqueue(it) }
        assertFalse(coordinator.deliverPending())

        assertEquals(requests, backend.posted)
        assertTrue(requests.all { store.marker(it.ledgerKey) == it.marker })
        assertTrue(store.pendingEntries.isEmpty())
        assertTrue(backend.active().size <= NotificationRetention.MAX_ACTIVE)
        assertTrue(backend.maxActive <= 50)
    }

    @Test
    fun foregroundNotificationId100IsNeverReclaimed() = runBlocking {
        val foreground = RetainedNotification(
            NotificationIdentity(null, 100), postedAt = 0, priority = -100, `protected` = true,
        )
        val store = FakeStore()
        val backend = FakeBackend(initial = listOf(foreground) + retainedMessages(49))
        val incoming = request(NotificationKind.COMPLETED, "done-1", seq = 21, projectId = "project-1")

        coordinator(store, backend).enqueue(incoming)
        coordinator(store, backend).deliverPending()

        assertFalse(foreground.identity in backend.cancelled)
        assertTrue(backend.active().any { it.identity == foreground.identity })
        assertEquals(incoming, backend.posted.single())
    }

    @Test
    fun hostsDeduplicateSameSeqReplayButAllowNewSeqAndIndependentHost() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend()
        val hostASeq7 = request(NotificationKind.MESSAGE, "message-1", hostId = "host-a", seq = 7, chatId = "chat")
        val hostANew = hostASeq7.copy(seq = 8, text = "new payload")
        val hostBSeq7 = hostASeq7.copy(hostId = "host-b")
        val coordinator = coordinator(store, backend)

        coordinator.enqueue(hostASeq7)
        coordinator.deliverPending()
        coordinator.enqueue(hostASeq7)
        assertFalse(coordinator.deliverPending())
        coordinator.enqueue(hostANew)
        coordinator.enqueue(hostBSeq7)
        assertFalse(coordinator.deliverPending())

        assertEquals(listOf(hostASeq7, hostANew, hostBSeq7), backend.posted)
        assertNotEquals(hostANew.identity, hostBSeq7.identity)
        assertEquals("new payload", backend.displayed(hostANew)?.text)
        assertEquals("host-a:message:message-1", hostANew.ledgerKey)
        assertEquals("host-b:message:message-1", hostBSeq7.ledgerKey)
    }

    @Test
    fun olderSeqReplayAfterShownDoesNotNotifyOrRegressMarker() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend()
        val newer = request(NotificationKind.COMPLETED, "done-ordered", seq = 82, projectId = "project-1")
        val older = newer.copy(seq = 81, text = "stale replay")
        val coordinator = coordinator(store, backend)

        coordinator.enqueue(newer)
        assertFalse(coordinator.deliverPending())
        coordinator.enqueue(older)
        assertFalse(coordinator.deliverPending())

        assertEquals(listOf(newer), backend.posted)
        assertEquals(newer.marker, store.marker(newer.ledgerKey))
        assertTrue(store.pendingEntries.isEmpty())
    }

    @Test
    fun olderSeqCannotReplaceNewerPendingDelivery() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend()
        val newer = request(NotificationKind.MESSAGE, "message-ordered", seq = 92, chatId = "chat-1", text = "new")
        val older = newer.copy(seq = 91, text = "old")
        val coordinator = coordinator(store, backend)

        coordinator.enqueue(newer)
        coordinator.enqueue(older)

        assertEquals(listOf(newer), store.pendingEntries)
        assertFalse(coordinator.deliverPending())
        assertEquals(listOf(newer), backend.posted)
        assertEquals(newer.marker, store.marker(newer.ledgerKey))
    }

    @Test
    fun silentDropDoesNotMarkAndDurableRestartRetriesDelivery() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend(silentDrops = 4)
        val request = request(NotificationKind.MESSAGE, "message-1", seq = 31, chatId = "chat-1")
        val coordinator = coordinator(store, backend)
        coordinator.enqueue(request)

        assertTrue(coordinator.deliverPending())
        assertEquals(null, store.marker(request.ledgerKey))
        assertEquals(listOf(request), store.pendingEntries)

        backend.silentDrops = 0
        val restarted = coordinator(store, backend)
        assertFalse(restarted.deliverPending())
        assertEquals(request.marker, store.marker(request.ledgerKey))
        assertTrue(store.pendingEntries.isEmpty())
        assertEquals(listOf(request), backend.posted)
    }

    @Test
    fun thrownPostLeavesDurablePendingForRecovery() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend(throws = 1)
        val request = request(NotificationKind.NEEDS_YOU, "needs-1", seq = 41)
        val coordinator = coordinator(store, backend)
        coordinator.enqueue(request)

        assertTrue(runCatching { coordinator.deliverPending() }.isFailure)
        assertEquals(null, store.marker(request.ledgerKey))
        assertEquals(listOf(request), store.pendingEntries)

        backend.throws = 0
        assertFalse(coordinator(store, backend).deliverPending())
        assertEquals(request.marker, store.marker(request.ledgerKey))
    }

    @Test
    fun failedPrepareDoesNotEvictExistingNotificationsAndRecoversWithCapacity() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend(seedCount = 50, prepareThrows = 1)
        val existing = backend.active().toSet()
        val request = request(NotificationKind.COMPLETED, "prepare-failure", seq = 111, projectId = "project-1")
        val coordinator = coordinator(store, backend)
        coordinator.enqueue(request)

        assertTrue(runCatching { coordinator.deliverPending() }.isFailure)
        assertEquals(existing, backend.active().toSet())
        assertTrue(backend.cancelled.isEmpty())
        assertEquals(listOf(request), store.pendingEntries)
        assertEquals(null, store.marker(request.ledgerKey))

        assertFalse(coordinator.deliverPending())
        assertEquals(NotificationRetention.MAX_ACTIVE, backend.active().size)
        assertEquals(request.marker, store.marker(request.ledgerKey))
        assertTrue(request in backend.posted)
    }

    @Test
    fun coalescedSameHostChatUsesLatestPayloadAndHostsRemainIndependent() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend()
        val hostAOld = request(NotificationKind.MESSAGE, "message-a1", hostId = "host-a", seq = 51, chatId = "chat")
        val hostANew = request(NotificationKind.MESSAGE, "message-a2", hostId = "host-a", seq = 52, chatId = "chat", text = "A latest")
        val hostBOld = request(NotificationKind.MESSAGE, "message-b1", hostId = "host-b", seq = 51, chatId = "chat")
        val hostBNew = request(NotificationKind.MESSAGE, "message-b2", hostId = "host-b", seq = 52, chatId = "chat", text = "B latest")
        val coordinator = coordinator(store, backend)

        listOf(hostAOld, hostANew, hostBOld, hostBNew).forEach { coordinator.enqueue(it) }
        assertFalse(coordinator.deliverPending())

        assertEquals("A latest", backend.displayed(hostANew)?.text)
        assertEquals("B latest", backend.displayed(hostBNew)?.text)
        assertNotEquals(hostANew.identity, hostBNew.identity)
    }

    @Test
    fun oldChatPayloadCannotAcknowledgeNewPayloadAfterSilentDrop() = runBlocking {
        val store = FakeStore()
        val old = request(NotificationKind.MESSAGE, "message-old", hostId = "host-a", seq = 61, chatId = "chat", text = "old")
        val incoming = request(NotificationKind.MESSAGE, "message-new", hostId = "host-a", seq = 62, chatId = "chat", text = "new")
        val backend = FakeBackend(silentDrops = 4)
        backend.seed(old)
        val coordinator = coordinator(store, backend)
        coordinator.enqueue(incoming)

        assertTrue(coordinator.deliverPending())
        assertEquals(null, store.marker(incoming.ledgerKey))
        assertEquals("old", backend.displayed(incoming)?.text)
    }

    @Test
    fun permissionOffRetainsPendingUntilPermissionReturns() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend(allowed = false)
        val request = request(NotificationKind.COMPLETED, "done-1", seq = 71, projectId = "project-1")
        val coordinator = coordinator(store, backend)
        coordinator.enqueue(request)

        assertTrue(coordinator.deliverPending())
        assertEquals(listOf(request), store.pendingEntries)
        assertEquals(null, store.marker(request.ledgerKey))
        assertTrue(backend.posted.isEmpty())

        backend.allowed = true
        assertFalse(coordinator(store, backend).deliverPending())
        assertEquals(request.marker, store.marker(request.ledgerKey))
    }

    @Test
    fun allProtectedEntriesRetainPendingAndNeverMarkUndeliveredRequest() = runBlocking {
        val foreground = RetainedNotification(
            NotificationIdentity("foreground", 100), postedAt = 0, `protected` = true,
        )
        val protectedEntries = listOf(foreground) + (0 until 49).map { index ->
            RetainedNotification(
                NotificationIdentity("protected-$index", index),
                postedAt = (index + 1).toLong(),
                `protected` = true,
            )
        }
        val store = FakeStore()
        val backend = FakeBackend(initial = protectedEntries)
        val request = request(NotificationKind.NEEDS_YOU, "needs-protected", seq = 101)
        val coordinator = coordinator(store, backend)

        coordinator.enqueue(request)
        assertTrue(coordinator.deliverPending())

        assertEquals(listOf(request), store.pendingEntries)
        assertEquals(null, store.marker(request.ledgerKey))
        assertTrue(backend.cancelled.isEmpty())
        assertTrue(backend.active().any { it.identity == foreground.identity })
        assertTrue(backend.posted.isEmpty())
    }

    @Test
    fun concurrentSubmissionsAreSerializedAndStayBelowQuota() = runBlocking {
        val store = FakeStore()
        val backend = FakeBackend(seedCount = 50)
        val coordinator = coordinator(store, backend)
        val requests = (0 until 100).map { index ->
            request(NotificationKind.MESSAGE, "message-$index", seq = 100L + index, chatId = "chat-$index")
        }

        requests.map { item ->
            async(Dispatchers.Default) { coordinator.enqueue(item) }
        }.awaitAll()
        listOf(
            async(Dispatchers.Default) { coordinator.deliverPending() },
            async(Dispatchers.Default) { coordinator.deliverPending() },
        ).awaitAll()

        assertTrue(store.pendingEntries.isEmpty())
        assertEquals(100, backend.posted.size)
        assertTrue(backend.maxActive <= 50)
        assertTrue(backend.active().size <= NotificationRetention.MAX_ACTIVE)
    }

    private fun coordinator(store: FakeStore, backend: FakeBackend): NotificationDeliveryCoordinator =
        NotificationDeliveryCoordinator(store, backend, pause = {})

    private fun request(
        kind: NotificationKind,
        id: String,
        hostId: String = "host-a",
        seq: Long? = null,
        projectId: String? = null,
        chatId: String? = null,
        text: String = id,
    ) = PendingNotification(
        kind = kind,
        hostId = hostId,
        id = id,
        title = kind.name,
        text = text,
        seq = seq,
        projectId = projectId,
        chatId = chatId,
        deepLinkKind = "chat",
        deepLinkId = chatId ?: projectId ?: id,
    )

    private fun retainedMessages(count: Int): List<RetainedNotification> = (0 until count).map { index ->
        RetainedNotification(NotificationIdentity("existing", index), postedAt = index.toLong())
    }

    private class FakeStore(initial: List<PendingNotification> = emptyList()) : NotificationDeliveryStore {
        var pendingEntries = initial.toMutableList()
        val marks = linkedMapOf<String, String>()

        override fun pending(): List<PendingNotification> = pendingEntries.toList()

        override fun savePending(value: List<PendingNotification>) {
            pendingEntries = value.toMutableList()
        }

        override fun marker(key: String): String? = marks[key]

        override fun markShown(key: String, marker: String) {
            marks[key] = marker
        }
    }

    private class FakeBackend(
        initial: List<RetainedNotification> = emptyList(),
        seedCount: Int = 0,
        var allowed: Boolean = true,
        var silentDrops: Int = 0,
        var throws: Int = 0,
        var prepareThrows: Int = 0,
    ) : NotificationDeliveryBackend {
        private val retained = linkedMapOf<NotificationIdentity, RetainedNotification>()
        private val displayedRequests = linkedMapOf<NotificationIdentity, PendingNotification>()
        val posted = mutableListOf<PendingNotification>()
        val cancelled = mutableListOf<NotificationIdentity>()
        var maxActive: Int = 0
            private set
        private var timestamp = 0L

        init {
            initial.forEach(::seed)
            repeat(seedCount) { index ->
                seed(RetainedNotification(NotificationIdentity("existing", index), postedAt = index.toLong()))
            }
            recordMax()
        }

        override fun allowed(): Boolean = allowed

        override fun prepare(request: PendingNotification) {
            if (prepareThrows > 0) {
                prepareThrows -= 1
                throw IllegalStateException("prepare failed")
            }
        }

        override fun active(): List<RetainedNotification> = retained.values.toList()

        override fun cancel(identity: NotificationIdentity) {
            cancelled += identity
            retained.remove(identity)
            displayedRequests.remove(identity)
        }

        override fun post(request: PendingNotification) {
            if (throws > 0) {
                throws -= 1
                throw IllegalStateException("post failed")
            }
            if (silentDrops > 0) {
                silentDrops -= 1
                return
            }
            val existing = retained[request.identity]
            if (existing == null && retained.size >= 50) return
            retained[request.identity] = RetainedNotification(request.identity, timestamp++)
            displayedRequests[request.identity] = request
            posted += request
            recordMax()
        }

        override fun shown(request: PendingNotification): Boolean {
            val displayed = displayedRequests[request.identity] ?: return false
            return displayed.ledgerKey == request.ledgerKey && displayed.marker == request.marker
        }

        fun seed(request: PendingNotification) {
            seed(RetainedNotification(request.identity, timestamp++))
            displayedRequests[request.identity] = request
        }

        fun seed(notification: RetainedNotification) {
            retained[notification.identity] = notification
            recordMax()
        }

        fun displayed(request: PendingNotification): PendingNotification? = displayedRequests[request.identity]

        private fun recordMax() {
            maxActive = maxOf(maxActive, retained.size)
        }
    }
}

package bot.mac.mobile.core.network

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import io.ktor.http.URLBuilder
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class NetworkPrimitivesTest {
    @Test
    fun endpointAcceptsHostAndPreservesExplicitTransport() {
        assertEquals("ws://10.0.2.2:7789/ws", EndpointBuilder.main("10.0.2.2:7789"))
        assertEquals("wss://bot.example/ws", EndpointBuilder.main("https://bot.example"))
        assertEquals("ws://bot.example/ws?x=1", EndpointBuilder.main("ws://bot.example?x=1"))
        assertTrue(EndpointBuilder.screen("ws://bot.example", "bot/a", "auto", "tab 1").contains("/ws/screen?"))
    }

    @Test
    fun endpointPreservesProxyPrefixAndDoesNotDuplicateWs() {
        assertEquals("wss://bot.example/proxy/ws", EndpointBuilder.main("https://bot.example/proxy"))
        assertEquals("wss://bot.example/proxy/ws", EndpointBuilder.main("wss://bot.example/proxy/ws"))
        assertTrue(EndpointBuilder.screen("wss://bot.example/proxy/ws", "bot", "auto", null)
            .startsWith("wss://bot.example/proxy/ws/screen?"))
        assertEquals("https://bot.example/proxy/api/v1", EndpointBuilder.http("wss://bot.example/proxy", "/api/v1"))
    }

    @Test
    fun endpointHandlesIpv6AndUtf8QueryValues() {
        assertEquals("ws://[::1]:7789/ws", EndpointBuilder.main("[::1]:7789"))
        val endpoint = EndpointBuilder.screen("https://bot.example?已有=值", "机器人/一", "auto mode", "标签 1")
        val parsed = URLBuilder(endpoint).build()
        assertEquals("值", parsed.parameters["已有"])
        assertEquals("机器人/一", parsed.parameters["bot_id"])
        assertEquals("标签 1", parsed.parameters["tab_id"])
    }

    @Test
    fun endpointRejectsUnknownScheme() {
        assertFailsWith<IllegalArgumentException> { EndpointBuilder.main("ftp://bot.example") }
    }

    @Test
    fun backoffIsExponentialAndCapped() {
        val policy = BackoffPolicy(FixedRandom(0.5))
        assertEquals(1_000L, policy.delayMillis(0))
        assertEquals(2_000L, policy.delayMillis(1))
        assertEquals(30_000L, policy.delayMillis(99))
    }

    @Test
    fun traceItemsAreSortedAndDeduplicated() {
        val merger = TraceCursorMerger()
        fun item(seq: Long) = buildJsonObject { put("aseq", seq); put("text", seq.toString()) }
        assertTrue(merger.add(item(3)))
        assertTrue(merger.add(item(1)))
        assertFalse(merger.add(item(3)))
        assertEquals(listOf(1L, 3L), merger.snapshot().mapNotNull { it.long("aseq") })
        assertEquals(1L, merger.firstAseq)
        assertEquals(3L, merger.lastAseq)
    }

    @Test
    fun lastSequenceNeverMovesBackwards() = kotlinx.coroutines.test.runTest {
        val store = InMemoryLastSeqStore()
        store.put("host", 8)
        store.put("host", 3)
        assertEquals(8L, store.get("host"))
        store.reset("host", 3)
        assertEquals(3L, store.get("host"))
    }

    @Test
    fun requestTrackerKeepsStableIdForWriteReplay() {
        val tracker = RequestTracker(SequenceIds())
        val first = tracker.register("chat.send", write = true)
        val replay = tracker.replayable().single()
        assertEquals(first.id, replay.frame.string("id"))
        assertEquals(first.clientRequestId, replay.frame.objectValue("params")?.string("client_request_id"))
        assertTrue(first.clientRequestId!!.matches(Regex("[0-9a-f-]{36}")))
        tracker.remove(first.id)
        assertEquals(0, tracker.size())
    }

    @Test
    fun requestTrackerSendsAtMostOncePerSocketSession() {
        val tracker = RequestTracker(SequenceIds())
        val request = tracker.register("chat.list")
        val firstSession = Any()
        assertTrue(tracker.markSent(request, firstSession))
        assertFalse(tracker.markSent(request, firstSession))
        assertTrue(tracker.markSent(request, Any()))
    }

    private class FixedRandom(private val value: Double) : kotlin.random.Random() {
        override fun nextBits(bitCount: Int): Int = (value * (1L shl bitCount)).toInt()
    }

    private class SequenceIds : IdGenerator {
        private var index = 0
        override fun nextId(): String = "00000000-0000-4000-8000-${(++index).toString().padStart(12, '0')}"
    }
}

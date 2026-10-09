package bot.mac.mobile.core.network

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
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
        assertTrue(endpoint.contains("%E6%9C%89%E6%95%B0=%E5%80%BC"))
        assertTrue(endpoint.contains("bot_id=%E6%9C%BA%E5%99%A8%2F%E4%B8%80"))
        assertTrue(endpoint.contains("tab_id=%E6%A0%87%E7%AD%BE%201"))
    }

    @Test
    fun endpointRejectsUnknownScheme() {
        assertFailsWith<IllegalArgumentException> { EndpointBuilder.main("ftp://bot.example") }
    }

    @Test
    fun backoffIsExponentialAndCapped() {
        val policy = BackoffPolicy(FixedRandom(0.0))
        assertEquals(1_000, policy.delayMillis(0))
        assertEquals(2_000, policy.delayMillis(1))
        assertEquals(30_000, policy.delayMillis(99))
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

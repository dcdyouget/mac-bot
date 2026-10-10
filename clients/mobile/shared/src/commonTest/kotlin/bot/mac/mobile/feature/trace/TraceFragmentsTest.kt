package bot.mac.mobile.feature.trace

import kotlin.test.Test
import kotlin.test.assertEquals

class TraceFragmentsTest {
    private val fragments = linkedMapOf(
        "stream:request:text" to "answer",
        "stream:request:thinking" to "reasoning",
        "stream:call:tool" to "output",
        "stream:other:text" to "unrelated",
        "stream:request:unknown" to "unknown",
    )

    @Test fun liveFragmentsRespectBothCollapsedChannels() {
        assertEquals(listOf("answer"), visibleTraceFragments(fragments, "request", "call", false, false))
        assertEquals(listOf("answer", "reasoning"), visibleTraceFragments(fragments, "request", "call", true, false))
        assertEquals(listOf("answer", "output"), visibleTraceFragments(fragments, "request", "call", false, true))
        assertEquals(listOf("answer", "reasoning", "output"), visibleTraceFragments(fragments, "request", "call", true, true))
    }

    @Test fun emptyOrPartialIdsDoNotMatchOtherRequests() {
        assertEquals(emptyList(), visibleTraceFragments(fragments, "", "", true, true))
        assertEquals(emptyList(), visibleTraceFragments(fragments, "quest", "all", true, true))
    }
}

package bot.mac.mobile.feature.memory

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class MemorySourceLogicTest {
    @Test
    fun sourceTimestampUsesMonthDayAndLocalTime() {
        val formatted = formatMemoryTimestamp("2026-10-09T10:21:45Z")

        assertTrue(Regex("\\d{2}/\\d{2} \\d{2}:\\d{2}").matches(formatted))
    }

    @Test
    fun malformedSourceTimestampRemainsReadable() {
        assertEquals("unknown", formatMemoryTimestamp("unknown"))
    }
}

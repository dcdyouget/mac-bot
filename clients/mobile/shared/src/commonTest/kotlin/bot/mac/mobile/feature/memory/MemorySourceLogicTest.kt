package bot.mac.mobile.feature.memory

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

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

    @Test
    fun sourceDateUsesEntryCreationTimeAfterLaterEdits() {
        val createdAt = "2026-10-09T10:21:45Z"
        val entry = buildJsonObject {
            put("created_at", createdAt)
            put("updated_at", "2026-10-12T18:35:10Z")
        }

        assertEquals(formatMemoryTimestamp(createdAt), memoryEntrySourceDate(entry))
    }
}

package bot.mac.mobile.feature.memory

import bot.mac.mobile.core.protocol.protocolJson
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileState
import bot.mac.mobile.core.state.StateReducer
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.jsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class MemoryListLogicTest {
    private fun json(value: String) = protocolJson.parseToJsonElement(value).jsonObject

    @Test
    fun memoryRowsAreOrderedByMostRecentUpdate() {
        val items = listOf(
            json("""{"id":"old","updated_at":"2026-10-09T08:00:00Z"}"""),
            json("""{"id":"new","updated_at":"2026-10-10T08:00:00Z"}"""),
        )

        assertEquals(listOf("new", "old"), sortMemoryEntries(items).map { it.str("id") })
    }

    @Test
    fun quotaColorChangesOnlyAboveNinetyPercentAndAtTheLimit() {
        assertEquals(MemoryQuotaLevel.NORMAL, memoryQuotaLevel(1_350, 1_500))
        assertEquals(MemoryQuotaLevel.WARNING, memoryQuotaLevel(1_351, 1_500))
        assertEquals(MemoryQuotaLevel.FULL, memoryQuotaLevel(1_500, 1_500))
    }

    @Test
    fun profileAndEntryLimitsCountEmojiAsOneUnicodeScalar() {
        assertEquals(1, "😀".unicodeScalarCount())
        assertEquals(20, ("😀".repeat(20)).unicodeScalarCount())
        assertTrue(("😀".repeat(21)).unicodeScalarCount() > 20)
    }

    @Test
    fun searchHighlightMarksOnlyMatchedText() {
        val highlighted = highlightMemoryText("I prefer dark mode", "dark", androidx.compose.ui.graphics.Color.Blue)

        assertEquals("dark", highlighted.getStringAnnotations("memory-search-hit", 0, highlighted.length).firstOrNull()?.item)
    }

    @Test
    fun realtimeUpdateAndDeleteReplaceTheVisibleEntryAndQuota() {
        val initial = MobileState(memoryEntries = listOf(json("""{"id":"entry-1","version":1,"content":"old"}""")))
        val updatedEntry = json("""{"id":"entry-1","version":2,"content":"new","updated_at":"2026-10-12T00:00:00Z"}""")
        val updated = StateReducer.event(initial, "memory.user.updated", buildJsonObject {
            put("entry", updatedEntry)
            put("quota", json("""{"used_chars":3,"limit_chars":1500}"""))
        })

        assertEquals("new", updated.memoryEntries.single().str("content"))
        assertEquals(3, updated.memoryQuota.long("used_chars"))

        val deleted = StateReducer.event(updated, "memory.user.deleted", buildJsonObject {
            put("entry_id", "entry-1")
            put("quota", json("""{"used_chars":0,"limit_chars":1500}"""))
        })
        assertTrue(deleted.memoryEntries.isEmpty())
        assertEquals(0, deleted.memoryQuota.long("used_chars"))
    }
}

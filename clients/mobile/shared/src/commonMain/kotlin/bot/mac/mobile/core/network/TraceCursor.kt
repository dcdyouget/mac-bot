package bot.mac.mobile.core.network

import kotlinx.serialization.json.JsonObject

/**
 * Merges trace.history and trace.item responses. aseq is the durable cursor;
 * duplicate items from the history/subscribe boundary are ignored.
 */
class TraceCursorMerger {
    private val items = mutableMapOf<Long, JsonObject>()

    var firstAseq: Long? = null
        private set
    var lastAseq: Long? = null
        private set

    fun add(item: JsonObject): Boolean {
        val aseq = item.long("aseq") ?: return false
        val inserted = items.put(aseq, item) == null
        firstAseq = items.keys.minOrNull()
        lastAseq = items.keys.maxOrNull()
        return inserted
    }

    fun addAll(values: Iterable<JsonObject>): Int = values.count { add(it) }
    fun snapshot(): List<JsonObject> = items.entries.sortedBy { it.key }.map { it.value }
    fun clear() {
        items.clear()
        firstAseq = null
        lastAseq = null
    }
}

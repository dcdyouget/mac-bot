package bot.mac.mobile.feature.memory

import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

/** Counts Unicode scalar values rather than UTF-16 code units, matching server limits. */
internal fun String.unicodeScalarCount(): Int {
    var count = 0
    var index = 0
    while (index < length) {
        val first = this[index]
        index += if (first.isHighSurrogate() && index + 1 < length && this[index + 1].isLowSurrogate()) 2 else 1
        count++
    }
    return count
}

internal enum class MemoryQuotaLevel { NORMAL, WARNING, FULL }

internal fun memoryQuotaLevel(used: Long, limit: Long): MemoryQuotaLevel = when {
    limit <= 0L || used >= limit -> MemoryQuotaLevel.FULL
    used.toDouble() / limit.toDouble() > 0.9 -> MemoryQuotaLevel.WARNING
    else -> MemoryQuotaLevel.NORMAL
}

internal fun sortMemoryEntries(entries: List<JsonObject>): List<JsonObject> = entries.sortedByDescending { it.str("updated_at") }

internal fun highlightMemoryText(content: String, query: String, highlightColor: androidx.compose.ui.graphics.Color): androidx.compose.ui.text.AnnotatedString =
    androidx.compose.ui.text.buildAnnotatedString {
        if (query.isBlank()) append(content) else {
            var cursor = 0
            while (cursor < content.length) {
                val index = content.indexOf(query, cursor, ignoreCase = true)
                if (index < 0) { append(content.substring(cursor)); break }
                append(content.substring(cursor, index))
                pushStringAnnotation("memory-search-hit", query)
                pushStyle(androidx.compose.ui.text.SpanStyle(background = highlightColor.copy(alpha = 0.18f), color = highlightColor))
                append(content.substring(index, index + query.length))
                pop()
                pop()
                cursor = index + query.length
            }
        }
    }

package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

/** Keeps ordinary blocks in place and groups only adjacent question blocks into a deck. */
internal fun groupAdjacentQuestions(blocks: List<JsonObject>): List<List<JsonObject>> {
    val groups = mutableListOf<MutableList<JsonObject>>()
    blocks.forEach { block ->
        val last = groups.lastOrNull()
        if (block.str("type") == "question" && last?.firstOrNull()?.str("type") == "question") {
            last += block
        } else {
            groups += mutableListOf(block)
        }
    }
    return groups
}

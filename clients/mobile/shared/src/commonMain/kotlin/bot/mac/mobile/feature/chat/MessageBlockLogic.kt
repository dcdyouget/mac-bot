package bot.mac.mobile.feature.chat

import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
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

/** Bot group messages expose only the user-facing collaboration nodes. */
internal fun renderedMessageBlocks(message: JsonObject, groupChat: Boolean): List<JsonObject> {
    val blocks = message.objects("blocks")
    if (!groupChat || message.obj("sender").str("kind") != "bot") return blocks
    val allowed = when (message.str("intent")) {
        "ack" -> setOf("text")
        "decision" -> setOf("text", "question")
        "done" -> setOf("completion")
        "blocked" -> setOf("blocked")
        "progress" -> emptySet()
        else -> emptySet()
    }
    return blocks.filter { it.str("type") in allowed }
}

internal fun hasUnknownMessageBlock(message: JsonObject): Boolean = message.arr("blocks").any { element ->
    val block = element as? JsonObject ?: return@any true
    block.str("type") !in knownMessageBlockTypes
}

internal fun shouldHideGroupNonProductMessage(message: JsonObject, groupChat: Boolean): Boolean {
    if (!groupChat || message.obj("sender").str("kind") != "bot" || hasUnknownMessageBlock(message)) return false
    val intent = message.str("intent")
    if (intent !in knownProductIntents) return false
    return intent == "progress" || (message.arr("blocks").isNotEmpty() && renderedMessageBlocks(message, groupChat).isEmpty())
}

internal fun shouldShowGroupFallback(message: JsonObject, groupChat: Boolean): Boolean =
    groupChat && message.obj("sender").str("kind") == "bot" && message.str("intent") !in knownProductIntents &&
        renderedMessageBlocks(message, groupChat).isEmpty() && !hasUnknownMessageBlock(message)

private val knownProductIntents = setOf("ack", "progress", "decision", "done", "blocked")

internal data class QuestionSelectionSummary(val letters: String, val labels: String)

internal fun questionSelectionSummary(options: List<String>, selectedIndexes: List<Int>): QuestionSelectionSummary {
    val valid = selectedIndexes.distinct().filter { it in options.indices }.sorted()
    return QuestionSelectionSummary(
        letters = valid.joinToString(", ") { optionLetter(it) },
        labels = valid.joinToString("、") { options[it] },
    )
}

internal fun optionLetter(index: Int): String {
    var value = index + 1
    val result = StringBuilder()
    while (value > 0) {
        value--
        result.append(('A'.code + value % 26).toChar())
        value /= 26
    }
    return result.reverse().toString()
}

internal fun projectTargetMessageId(
    projectId: String,
    requestMessageId: String?,
    assignments: List<JsonObject>,
): String? {
    val projectAssignments = assignments.filter { it.str("project_id") == projectId && projectId.isNotBlank() }
    if (projectAssignments.isEmpty()) return null

    val roots = requestMessageId?.takeIf(String::isNotBlank)?.let { messageId ->
        projectAssignments.filter { it.str("trigger_message_id") == messageId }
    }.orEmpty()
    val reachable = mutableListOf<Pair<JsonObject, Int>>()
    var frontier = roots
    var depth = 0
    val visited = mutableSetOf<String>()
    while (frontier.isNotEmpty() && depth < projectAssignments.size + 1) {
        val fresh = frontier.filter { visited.add(it.str("id")) }
        reachable += fresh.map { it to depth }
        val parentIds = fresh.map { it.str("id") }.filter(String::isNotBlank).toSet()
        frontier = projectAssignments.filter { it.str("parent_assignment_id") in parentIds }
        depth++
    }

    val candidates = (reachable.takeIf { it.isNotEmpty() } ?: projectAssignments.map { it to 0 })
        .filter { (assignment, _) -> assignment.str("status") == "done" && assignment.str("result_message_id").isNotBlank() }
    return candidates.maxWithOrNull(
        compareBy<Pair<JsonObject, Int>> { it.second }
            .thenBy { it.first.str("finished_at").ifBlank { it.first.str("created_at") } },
    )?.first?.str("result_message_id")?.takeIf(String::isNotBlank)
}

internal val knownMessageBlockTypes = setOf(
    "text", "image", "file", "task_card", "completion", "progress", "blocked", "question", "project_card", "review_card",
    "delegation", "approval", "approval_ref", "takeover_request", "bot_dm_ref", "system", "loop_paused", "link_preview",
)

package bot.mac.mobile.feature.group

import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.str
import kotlinx.serialization.json.JsonObject

internal data class GroupAvatarLayout(
    val botIds: List<String>,
    val overflowCount: Int,
    val totalCount: Int,
)

internal data class GroupMemberTaskState(
    val status: String,
    val assignmentId: String?,
)

/** The lead occupies the upper-left cell; the remaining members retain join order. */
internal fun groupAvatarLayout(members: List<JsonObject>, bots: List<JsonObject>): GroupAvatarLayout {
    val botsById = bots.associateBy { it.str("id") }
    val unique = members.mapIndexedNotNull { index, member ->
        val id = member.str("bot_id").takeIf(String::isNotBlank) ?: return@mapIndexedNotNull null
        Triple(index, id, member)
    }.distinctBy { it.second }
    val hasJoinTimes = unique.all { (_, _, member) -> member.str("joined_at").isNotBlank() }
    val ordered = unique.sortedWith(
        compareBy<Triple<Int, String, JsonObject>> { !botsById[it.second].let { bot -> bot?.boolean("is_main") == true } }
            .thenBy { if (hasJoinTimes) it.third.str("joined_at") else it.first.toString().padStart(8, '0') },
    )
    val overflow = (ordered.size - 3).coerceAtLeast(0).takeIf { ordered.size > 4 } ?: 0
    val visibleCount = if (overflow > 0) 3 else minOf(ordered.size, 4)
    return GroupAvatarLayout(
        botIds = ordered.take(visibleCount).map { it.second },
        overflowCount = overflow,
        totalCount = ordered.size,
    )
}

internal fun groupMemberTaskState(
    projectId: String,
    member: JsonObject,
    assignments: List<JsonObject>,
    bots: List<JsonObject>,
): GroupMemberTaskState {
    val botId = member.str("bot_id")
    val projectAssignments = assignments.filter {
        it.str("bot_id") == botId && (projectId.isBlank() || it.str("project_id") == projectId)
    }
    val currentId = member.str("current_assignment_id").takeIf(String::isNotBlank)
    val currentAssignment = currentId?.let { id -> projectAssignments.firstOrNull { it.str("id") == id } }
    val latestAssignment = currentAssignment ?: projectAssignments
        .sortedByDescending { it.str("started_at").ifBlank { it.str("created_at") } }
        .firstOrNull { it.str("status") in ACTIVE_ASSIGNMENT_STATES }
        ?: projectAssignments.maxByOrNull { it.str("finished_at").ifBlank { it.str("created_at") } }
    val status = member.str("state").takeIf(String::isNotBlank)
        ?: latestAssignment?.str("status")?.takeIf(String::isNotBlank)
        ?: bots.firstOrNull { it.str("id") == botId }?.let { it.obj("status").str("summary") }
            ?.takeIf(String::isNotBlank)
        ?: "idle"
    return GroupMemberTaskState(status, currentId ?: latestAssignment?.str("id")?.takeIf(String::isNotBlank))
}

private val ACTIVE_ASSIGNMENT_STATES = setOf("queued", "working", "waiting_user", "waiting_bot", "blocked")

package bot.mac.mobile.feature.group

import bot.mac.mobile.core.protocol.protocolJson
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject

class GroupMemberLogicTest {
    private fun objects(value: String): List<JsonObject> =
        protocolJson.parseToJsonElement(value).jsonArray.map { it.jsonObject }

    @Test
    fun groupAvatarPutsLeadFirstAndReservesFourthCellForOverflow() {
        val bots = objects(
            """[
                {"id":"worker-1","is_main":false},
                {"id":"main","is_main":true},
                {"id":"worker-2","is_main":false},
                {"id":"worker-3","is_main":false},
                {"id":"worker-4","is_main":false}
            ]""",
        )
        val members = objects(
            """[
                {"bot_id":"worker-1","joined_at":"2026-01-02T00:00:00Z"},
                {"bot_id":"worker-2","joined_at":"2026-01-03T00:00:00Z"},
                {"bot_id":"worker-3","joined_at":"2026-01-04T00:00:00Z"},
                {"bot_id":"worker-4","joined_at":"2026-01-05T00:00:00Z"},
                {"bot_id":"main","joined_at":"2025-12-01T00:00:00Z"}
            ]""",
        )

        val layout = groupAvatarLayout(members, bots)

        assertEquals(listOf("main", "worker-1", "worker-2"), layout.botIds)
        assertEquals(2, layout.overflowCount)
        assertEquals(5, layout.totalCount)
    }

    @Test
    fun memberUsesItsCurrentAssignmentOrFallsBackToTheLatestActiveTask() {
        val member = objects("""[{"bot_id":"worker","state":"idle"}]""").single()
        val assignments = objects(
            """[
                {"id":"old","project_id":"p","bot_id":"worker","status":"done","finished_at":"2026-01-01T00:00:00Z"},
                {"id":"current","project_id":"p","bot_id":"worker","status":"waiting_user","started_at":"2026-01-02T00:00:00Z"},
                {"id":"other-project","project_id":"other","bot_id":"worker","status":"working","started_at":"2026-01-03T00:00:00Z"}
            ]""",
        )

        val result = groupMemberTaskState("p", member, assignments, emptyList())

        assertEquals("idle", result.status)
        assertEquals("current", result.assignmentId)
    }

    @Test
    fun missingMemberStateIsDerivedFromItsProjectAssignment() {
        val member = objects("""[{"bot_id":"worker"}]""").single()
        val assignments = objects("""[{"id":"a","project_id":"p","bot_id":"worker","status":"working","started_at":"2026-01-02T00:00:00Z"}]""")

        val result = groupMemberTaskState("p", member, assignments, emptyList())

        assertEquals("working", result.status)
        assertEquals("a", result.assignmentId)
    }
}

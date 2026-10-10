package bot.mac.mobile.feature.chat

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals

class ReviewCardLogicTest {
    @Test
    fun usesProjectNameFromStateForProtocolReviewCard() {
        val block = buildJsonObject { put("type", "review_card"); put("project_id", "project-1") }
        val projects = listOf(buildJsonObject { put("id", "project-1"); put("name", "登录功能") })

        assertEquals("登录功能", reviewProjectLabel(block, projects))
    }

    @Test
    fun fallsBackToProjectIdWhenProjectIsMissingOrNameIsBlank() {
        val missing = buildJsonObject { put("type", "review_card"); put("project_id", "deleted-project") }
        assertEquals("deleted-project", reviewProjectLabel(missing, emptyList<JsonObject>()))

        val blankBlock = buildJsonObject { put("type", "review_card"); put("project_id", "project-1") }
        val blank = buildJsonObject { put("id", "project-1"); put("name", "") }
        assertEquals("project-1", reviewProjectLabel(blankBlock, listOf(blank)))
    }
}

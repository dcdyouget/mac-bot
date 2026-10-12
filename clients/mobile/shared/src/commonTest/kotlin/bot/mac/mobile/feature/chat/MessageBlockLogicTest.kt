package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import bot.mac.mobile.core.protocol.str

class MessageBlockLogicTest {
    @Test
    fun onlyAdjacentQuestionBlocksBecomeADeckAndOtherBlocksKeepTheirPositions() {
        val blocks = Json.parseToJsonElement(
            """[
                {"type":"text","markdown":"before"},
                {"type":"question","question_id":"q1"},
                {"type":"question","question_id":"q2"},
                {"type":"text","markdown":"between"},
                {"type":"question","question_id":"q3"}
            ]""",
        ).jsonArray.map { it.jsonObject }

        val runs = groupAdjacentQuestions(blocks)
        assertEquals(listOf("text", "question,question", "text", "question"), runs.map { run -> run.joinToString(",") { it.str("type") } })
        assertEquals("q1", runs[1].first().str("question_id"))
        assertEquals("q3", runs.last().single().str("question_id"))
    }
}

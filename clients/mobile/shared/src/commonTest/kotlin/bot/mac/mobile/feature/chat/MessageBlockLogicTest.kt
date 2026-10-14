package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
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

    @Test
    fun groupMessagesRenderOnlyTheUserFacingProductNodes() {
        val message = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"done","blocks":[
                {"type":"progress","text":"internal"},
                {"type":"completion","summary":"ready"},
                {"type":"future_node"}
            ]}""",
        ).jsonObject

        assertEquals(listOf("completion"), renderedMessageBlocks(message, groupChat = true).map { it.str("type") })
        assertEquals(listOf("progress", "completion", "future_node"), renderedMessageBlocks(message, groupChat = false).map { it.str("type") })
        assertTrue(hasUnknownMessageBlock(message))
        assertFalse(shouldHideGroupNonProductMessage(message, groupChat = true))
    }

    @Test
    fun decisionTextAndQuestionBlocksRemainVisibleButProgressOnlyMessagesAreHidden() {
        val decision = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"decision","blocks":[{"type":"text"},{"type":"question"}]}""",
        ).jsonObject
        val progress = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"progress","blocks":[{"type":"progress"}]}""",
        ).jsonObject
        val progressWithoutBlocks = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"progress","fallback_text":"internal"}""",
        ).jsonObject
        val futureIntent = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"future_intent","fallback_text":"compatibility text","blocks":[{"type":"text","markdown":"not classified"}]}""",
        ).jsonObject

        assertEquals(listOf("text", "question"), renderedMessageBlocks(decision, groupChat = true).map { it.str("type") })
        assertTrue(shouldHideGroupNonProductMessage(progress, groupChat = true))
        assertTrue(shouldHideGroupNonProductMessage(progressWithoutBlocks, groupChat = true))
        assertTrue(renderedMessageBlocks(futureIntent, groupChat = true).isEmpty())
        assertTrue(shouldShowGroupFallback(futureIntent, groupChat = true))
    }

    @Test
    fun selectedQuestionOptionsProduceStableLetterAndLabelSummary() {
        assertEquals(QuestionSelectionSummary("A, C", "First、Third"), questionSelectionSummary(listOf("First", "Second", "Third"), listOf(2, 0, 2, 8)))
    }

    @Test
    fun projectCardTargetFollowsTheAssignmentChainFromTheQuotedRequest() {
        val assignments = Json.parseToJsonElement(
            """[
                {"id":"root","project_id":"p","trigger_message_id":"request","status":"done","parent_assignment_id":null,"result_message_id":"first-result","finished_at":"2026-01-01T00:00:00Z"},
                {"id":"last","project_id":"p","trigger_message_id":"handoff","status":"done","parent_assignment_id":"root","result_message_id":"final-result","finished_at":"2026-01-02T00:00:00Z"},
                {"id":"other","project_id":"q","status":"done","result_message_id":"wrong-project","finished_at":"2026-01-03T00:00:00Z"}
            ]""",
        ).jsonArray.map { it.jsonObject }

        assertEquals("final-result", projectTargetMessageId("p", "request", assignments))
        assertEquals("final-result", projectTargetMessageId("p", null, assignments))
        assertEquals(null, projectTargetMessageId("missing", "request", assignments))
    }
}

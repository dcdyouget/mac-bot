package bot.mac.mobile.feature.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
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
    fun streamingGroupProgressMessageCanShowItsTextDelta() {
        val streaming = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"progress","streaming":true,"blocks":[{"type":"text","markdown":"working"},{"type":"progress","text":"internal"}]}""",
        ).jsonObject

        assertEquals(listOf("text"), renderedMessageBlocks(streaming, groupChat = true).map { it.str("type") })
        assertFalse(shouldHideGroupNonProductMessage(streaming, groupChat = true))
    }

    @Test
    fun groupMessagesKeepPublicAttachmentsInStreamingAndFinalStates() {
        for (streaming in listOf(true, false)) {
            val message = Json.parseToJsonElement(
                """{"sender":{"kind":"bot"},"intent":"ack","streaming":$streaming,"blocks":[{"type":"image"},{"type":"file"},{"type":"link_preview"},{"type":"progress"}]}""",
            ).jsonObject

            assertEquals(
                listOf("image", "file", "link_preview"),
                renderedMessageBlocks(message, groupChat = true).map { it.str("type") },
            )
            assertFalse(shouldHideGroupNonProductMessage(message, groupChat = true))
        }
    }

    @Test
    fun publicAttachmentsDoNotExposeAnInternalProgressBlock() {
        val message = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"progress","streaming":false,"blocks":[{"type":"progress","text":"internal"},{"type":"image"},{"type":"file"},{"type":"link_preview"}]}""",
        ).jsonObject

        assertEquals(
            listOf("image", "file", "link_preview"),
            renderedMessageBlocks(message, groupChat = true).map { it.str("type") },
        )
        assertFalse(shouldHideGroupNonProductMessage(message, groupChat = true))

        val internalOnly = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"intent":"progress","streaming":false,"blocks":[{"type":"progress","text":"internal"}]}""",
        ).jsonObject
        assertTrue(shouldHideGroupNonProductMessage(internalOnly, groupChat = true))
    }

    @Test
    fun streamingPlaceholderIndicatorAppearsOnlyWhenNoRenderedContentExists() {
        val empty = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"streaming":true,"fallback_text":"","blocks":[]}""",
        ).jsonObject
        val card = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"streaming":true,"fallback_text":"","blocks":[{"type":"completion","summary":"done"}]}""",
        ).jsonObject
        val final = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"streaming":false,"fallback_text":"","blocks":[]}""",
        ).jsonObject

        assertTrue(shouldShowStreamingPlaceholder(empty, groupChat = false))
        assertFalse(shouldShowStreamingPlaceholder(card, groupChat = false))
        assertFalse(shouldShowStreamingPlaceholder(final, groupChat = false))
    }

    @Test
    fun emptyAndWhitespaceTextBlocksStillShowDmAndGroupStreamingPlaceholder() {
        val values = listOf("", " \n\t ")
        for (groupChat in listOf(false, true)) {
            values.forEach { markdown ->
                val message = buildJsonObject {
                    putJsonObject("sender") { put("kind", "bot") }
                    if (groupChat) put("intent", "ack")
                    put("streaming", true)
                    put("fallback_text", "")
                    put("blocks", buildJsonArray {
                        add(buildJsonObject { put("type", "text"); put("markdown", markdown) })
                    })
                }
                assertTrue(shouldShowStreamingPlaceholder(message, groupChat), "groupChat=$groupChat markdown=$markdown")
            }
        }
    }

    @Test
    fun textBlockTextFieldAlsoCountsAsVisibleContent() {
        val message = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"streaming":true,"fallback_text":"","blocks":[{"type":"text","text":"visible"}]}""",
        ).jsonObject

        assertFalse(shouldShowStreamingPlaceholder(message, groupChat = false))
    }

    @Test
    fun streamingPlaceholderRequiresAnUndeletedBotMessage() {
        val user = Json.parseToJsonElement(
            """{"sender":{"kind":"user"},"streaming":true,"fallback_text":"","blocks":[]}""",
        ).jsonObject
        val deletedBot = Json.parseToJsonElement(
            """{"sender":{"kind":"bot"},"deleted":true,"streaming":true,"fallback_text":"","blocks":[]}""",
        ).jsonObject

        assertFalse(shouldShowStreamingPlaceholder(user, groupChat = false))
        assertFalse(shouldShowStreamingPlaceholder(deletedBot, groupChat = false))
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

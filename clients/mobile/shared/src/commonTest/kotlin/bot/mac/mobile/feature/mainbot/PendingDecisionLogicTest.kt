package bot.mac.mobile.feature.mainbot

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import kotlinx.coroutines.test.runTest

class PendingDecisionLogicTest {
    @Test
    fun mainBotGetsAllDecisionKindsAndSortedNewestFirst() {
        val decisions = pendingDecisionsForContext(
            approvals = listOf(buildJsonObject { put("id", "a"); put("state", "pending"); put("chat_id", "bot-chat"); put("created_at", "2026-10-13T11:00:00Z") }),
            questions = listOf(buildJsonObject { put("id", "q"); put("state", "pending"); put("chat_id", "group-chat"); put("text", "Choose") }),
            projects = listOf(buildJsonObject { put("id", "p"); put("chat_id", "group-chat"); put("status", "pending_review"); put("name", "Review") }),
            chats = emptyList(),
            assignments = emptyList(),
            context = PendingDecisionContext(chatId = "main", isMain = true),
        )

        assertEquals(listOf("a", "q", "p"), decisions.map { it.id })
        assertEquals(listOf(PendingDecisionKind.APPROVAL, PendingDecisionKind.QUESTION, PendingDecisionKind.REVIEW), decisions.map { it.kind })
    }

    @Test
    fun botChatCollectsItsApprovalsQuestionsAndReviewsAcrossChats() {
        val decisions = pendingDecisionsForContext(
            approvals = listOf(
                buildJsonObject { put("id", "own-approval"); put("state", "pending"); put("chat_id", "other-dm"); put("bot_id", "worker") },
                buildJsonObject { put("id", "other-approval"); put("state", "pending"); put("chat_id", "other-dm"); put("bot_id", "other") },
            ),
            questions = listOf(buildJsonObject { put("id", "own-question"); put("state", "pending"); put("chat_id", "group"); put("bot_id", "worker") }),
            projects = listOf(
                buildJsonObject { put("id", "member-project"); put("chat_id", "member-group"); put("status", "review"); put("members", buildJsonArray { add(buildJsonObject { put("bot_id", "worker") }) }) },
                buildJsonObject { put("id", "unrelated-project"); put("chat_id", "other-group"); put("status", "review") },
            ),
            chats = emptyList(),
            assignments = emptyList(),
            context = PendingDecisionContext(chatId = "worker-dm", botId = "worker"),
        )

        assertEquals(setOf("own-approval", "own-question", "member-project"), decisions.map { it.id }.toSet())
    }

    @Test
    fun groupContextIncludesOnlyItsPendingItems() {
        val decisions = pendingDecisionsForContext(
            approvals = listOf(
                buildJsonObject { put("id", "group-approval"); put("state", "pending"); put("chat_id", "group-chat") },
                buildJsonObject { put("id", "other-approval"); put("state", "pending"); put("chat_id", "other-chat") },
            ),
            questions = emptyList(),
            projects = listOf(
                buildJsonObject { put("id", "project"); put("status", "review"); put("chat_id", "group-chat") },
                buildJsonObject { put("id", "other-project"); put("status", "review"); put("chat_id", "other-chat") },
            ),
            chats = emptyList(),
            assignments = emptyList(),
            context = PendingDecisionContext(chatId = "group-chat", projectId = "project"),
        )

        assertEquals(setOf("group-approval", "project"), decisions.map { it.id }.toSet())
    }

    @Test
    fun blankChatIdDoesNotAccidentallyMatchUnscopedDecisions() {
        val decisions = pendingDecisionsForContext(
            approvals = listOf(buildJsonObject { put("id", "unscoped"); put("state", "pending") }),
            questions = listOf(buildJsonObject { put("id", "unscoped-question"); put("state", "pending") }),
            projects = emptyList(),
            chats = emptyList(),
            assignments = emptyList(),
            context = PendingDecisionContext(chatId = "", projectId = "project-a"),
        )

        assertEquals(emptyList(), decisions)
    }

    @Test
    fun decisionMatchesItsSpecificMessageBlock() {
        val decision = PendingDecision(PendingDecisionKind.QUESTION, "q1", "chat", "bot", "", "Pick")
        val target = buildJsonObject {
            putJsonObject("sender") { put("kind", "bot") }
            put("blocks", buildJsonArray { add(buildJsonObject { put("type", "question"); put("question_id", "q1") }) })
        }
        val other = buildJsonObject {
            put("blocks", buildJsonArray { add(buildJsonObject { put("type", "question"); put("question_id", "q2") }) })
        }

        assertTrue(decision.matchesMessage(target))
        assertFalse(decision.matchesMessage(other))
    }

    @Test
    fun approvalReferenceChatOverridesApprovalRecordAndReferenceMatches() {
        val decisions = pendingDecisionsForContext(
            approvals = listOf(buildJsonObject {
                put("id", "approval-1")
                put("state", "pending")
                put("chat_id", "stale-chat")
                put("project_id", "project-1")
            }),
            questions = emptyList(),
            projects = emptyList(),
            chats = emptyList(),
            assignments = emptyList(),
            context = PendingDecisionContext(chatId = "group-chat", projectId = "project-1"),
            referencedApprovalIds = setOf("approval-1"),
            referencedApprovalChats = mapOf("approval-1" to "approval-chat"),
        )
        val decision = decisions.single()
        val referenceMessage = buildJsonObject {
            put("id", "reference-message")
            put("blocks", buildJsonArray {
                add(buildJsonObject {
                    put("type", "approval_ref")
                    put("approval_id", "approval-1")
                    put("chat_id", "approval-chat")
                })
            })
        }
        val directApprovalMessage = buildJsonObject {
            put("blocks", buildJsonArray {
                add(buildJsonObject { put("type", "approval"); put("approval_id", "approval-1") })
            })
        }

        assertEquals("approval-chat", decision.chatId)
        assertTrue(decision.matchesMessage(referenceMessage))
        assertTrue(decision.matchesMessage(directApprovalMessage))
    }

    @Test
    fun approvalSearchPagesBackUntilItFindsReference() = runTest {
        val decision = PendingDecision(PendingDecisionKind.APPROVAL, "approval-2", "approval-chat", "bot", "", "Approve")
        val cursors = mutableListOf<Long?>()
        val found = findPendingDecisionMessageIdInHistory(decision, emptyList()) { beforeSeq ->
            cursors += beforeSeq
            if (beforeSeq == null) {
                PendingDecisionHistoryPage(
                    messages = listOf(
                        buildJsonObject { put("id", "recent-2"); put("seq", 2) },
                        buildJsonObject { put("id", "recent-1"); put("seq", 1) },
                    ),
                    hasMore = true,
                )
            } else {
                PendingDecisionHistoryPage(
                    messages = listOf(buildJsonObject {
                        put("id", "reference-message")
                        put("seq", 0)
                        put("blocks", buildJsonArray {
                            add(buildJsonObject { put("type", "approval_ref"); put("approval_id", "approval-2") })
                        })
                    }),
                    hasMore = false,
                )
            }
        }

        assertEquals("reference-message", found)
        assertEquals(listOf(null, 1L), cursors)
    }
}

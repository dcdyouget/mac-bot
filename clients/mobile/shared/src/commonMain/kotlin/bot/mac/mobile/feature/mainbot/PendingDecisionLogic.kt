package bot.mac.mobile.feature.mainbot

import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.boolean
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.JsonObject

internal enum class PendingDecisionKind { APPROVAL, QUESTION, REVIEW }

internal data class PendingDecision(
    val kind: PendingDecisionKind,
    val id: String,
    val chatId: String,
    val botId: String,
    val projectId: String,
    val title: String,
    val tool: String = "",
    val risk: String = "",
    val createdAt: String = "",
)

internal data class PendingDecisionContext(
    val chatId: String,
    val botId: String? = null,
    val projectId: String? = null,
    val isMain: Boolean = false,
)

internal fun pendingDecisionsForContext(
    approvals: List<JsonObject>,
    questions: List<JsonObject>,
    projects: List<JsonObject>,
    chats: List<JsonObject>,
    assignments: List<JsonObject>,
    context: PendingDecisionContext,
    referencedApprovalIds: Set<String> = emptySet(),
    referencedApprovalChats: Map<String, String> = emptyMap(),
): List<PendingDecision> {
    val assignmentById = assignments.associateBy { it.str("id") }
    val decisions = buildList {
        approvals.forEach { approval ->
            if (approval.str("state") != "pending") return@forEach
            val assignment = assignmentById[approval.str("assignment_id")]
            val projectId = approval.str("project_id").ifBlank { assignment?.str("project_id").orEmpty() }
            val matches = context.isMain || (context.chatId.isNotBlank() && approval.str("chat_id") == context.chatId) ||
                (context.botId != null && (approval.str("bot_id") == context.botId || assignment?.str("bot_id") == context.botId)) ||
                (context.projectId != null && projectId == context.projectId) ||
                approval.str("id") in referencedApprovalIds
            if (matches) add(
                PendingDecision(
                    kind = PendingDecisionKind.APPROVAL,
                    id = approval.str("id"),
                    chatId = referencedApprovalChats[approval.str("id")]
                        ?.takeIf(String::isNotBlank)
                        ?: approval.str("chat_id"),
                    botId = approval.str("bot_id").ifBlank { assignment?.str("bot_id").orEmpty() },
                    projectId = projectId,
                    title = approval.str("summary").ifBlank { approval.str("tool") },
                    tool = approval.str("tool"),
                    risk = approval.str("risk"),
                    createdAt = approval.str("created_at"),
                ),
            )
        }

        questions.forEach { question ->
            if (question.str("state") != "pending") return@forEach
            val assignment = assignmentById[question.str("assignment_id")]
            val projectId = question.str("project_id").ifBlank { assignment?.str("project_id").orEmpty() }
            val matches = context.isMain || (context.chatId.isNotBlank() && question.str("chat_id") == context.chatId) ||
                (context.botId != null && (question.str("bot_id") == context.botId || assignment?.str("bot_id") == context.botId)) ||
                (context.projectId != null && projectId == context.projectId)
            if (matches) add(
                PendingDecision(
                    kind = PendingDecisionKind.QUESTION,
                    id = question.str("id"),
                    chatId = question.str("chat_id"),
                    botId = question.str("bot_id").ifBlank { assignment?.str("bot_id").orEmpty() },
                    projectId = projectId,
                    title = question.str("text"),
                    createdAt = question.str("created_at"),
                ),
            )
        }

        projects.forEach { project ->
            if (project.str("status") !in setOf("review", "pending_review")) return@forEach
            val projectId = project.str("id")
            val projectChatId = project.str("chat_id").ifBlank {
                chats.firstOrNull { it.str("project_id") == projectId }?.str("id").orEmpty()
            }
            val memberIds = project.arr("members").mapNotNull { (it as? JsonObject)?.str("bot_id")?.takeIf(String::isNotBlank) }
            val botHasAssignment = context.botId != null && assignments.any {
                it.str("project_id") == projectId && it.str("bot_id") == context.botId
            }
            val matches = context.isMain || context.projectId == projectId ||
                (context.botId != null && (project.str("lead_bot_id") == context.botId || context.botId in memberIds || botHasAssignment))
            if (matches) add(
                PendingDecision(
                    kind = PendingDecisionKind.REVIEW,
                    id = projectId,
                    chatId = projectChatId,
                    botId = project.str("lead_bot_id"),
                    projectId = projectId,
                    title = project.str("name").ifBlank { projectId },
                    createdAt = project.str("updated_at").ifBlank { project.str("created_at") },
                ),
            )
        }
    }
    return decisions.distinctBy { it.kind to it.id }.sortedWith(
        compareByDescending<PendingDecision> { it.createdAt }
            .thenBy { it.kind.ordinal }
            .thenBy { it.id },
    )
}

internal fun PendingDecision.matchesMessage(message: JsonObject): Boolean {
    val blocks = message.objects("blocks")
    return blocks.any { block ->
        when (kind) {
            PendingDecisionKind.APPROVAL -> block.str("type") in setOf("approval", "approval_ref") && block.str("approval_id") == id
            PendingDecisionKind.QUESTION -> block.str("type") == "question" && block.str("question_id") == id
            PendingDecisionKind.REVIEW -> block.str("type") == "review_card" && block.str("project_id") == projectId
        }
    }
}

/** Finds the exact card behind a sidebar decision, paging older chat history when needed. */
internal suspend fun findPendingDecisionMessageId(
    repository: MobileRepository,
    decision: PendingDecision,
): String? {
    if (decision.chatId.isBlank()) return null
    val cachedMessages = repository.state.value.messages[decision.chatId].orEmpty()
    return findPendingDecisionMessageIdInHistory(decision, cachedMessages) { beforeSeq ->
        val result = repository.call("chat.history", buildJsonObject {
            put("chat_id", decision.chatId)
            put("limit", 100)
            beforeSeq?.let { put("before_seq", it) }
        })
        PendingDecisionHistoryPage(result.objects("messages"), result.boolean("has_more"))
    }
}

internal data class PendingDecisionHistoryPage(
    val messages: List<JsonObject>,
    val hasMore: Boolean,
)

/** Searches the cached tail and then pages backwards until the decision card is found or history ends. */
internal suspend fun findPendingDecisionMessageIdInHistory(
    decision: PendingDecision,
    cachedMessages: List<JsonObject>,
    fetchPage: suspend (beforeSeq: Long?) -> PendingDecisionHistoryPage,
): String? {
    cachedMessages.firstOrNull(decision::matchesMessage)?.str("id")?.takeIf(String::isNotBlank)?.let { return it }
    var beforeSeq: Long? = null
    repeat(100) {
        val page = fetchPage(beforeSeq)
        val messages = page.messages
        messages.firstOrNull(decision::matchesMessage)?.str("id")?.takeIf(String::isNotBlank)?.let { return it }
        if (!page.hasMore || messages.isEmpty()) return null
        val oldestSeq = messages.minOfOrNull { it.long("seq") }?.takeIf { it > 0 } ?: return null
        if (oldestSeq == beforeSeq) return null
        beforeSeq = oldestSeq
    }
    return null
}

package bot.mac.mobile.feature.mainbot

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.protocol.objects
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.MobileState
import bot.mac.mobile.resources.Res
import bot.mac.mobile.resources.followup_current_pending
import bot.mac.mobile.resources.followup_details
import bot.mac.mobile.resources.followup_error
import bot.mac.mobile.resources.followup_groups
import bot.mac.mobile.resources.followup_handle
import bot.mac.mobile.resources.followup_loading
import bot.mac.mobile.resources.followup_no_groups
import bot.mac.mobile.resources.followup_no_pending
import bot.mac.mobile.resources.followup_no_tasks
import bot.mac.mobile.resources.followup_offline
import bot.mac.mobile.resources.followup_open_chat
import bot.mac.mobile.resources.followup_queue_count
import bot.mac.mobile.resources.followup_review_action
import bot.mac.mobile.resources.followup_tasks
import bot.mac.mobile.resources.followup_title
import bot.mac.mobile.resources.followup_usage_tokens
import bot.mac.mobile.resources.feature_back
import bot.mac.mobile.resources.feature_status_blocked
import bot.mac.mobile.resources.feature_status_done
import bot.mac.mobile.resources.feature_status_queued
import bot.mac.mobile.resources.feature_status_review
import bot.mac.mobile.resources.feature_status_waiting_bot
import bot.mac.mobile.resources.feature_status_waiting_user
import bot.mac.mobile.resources.feature_status_working
import bot.mac.mobile.resources.feature_status_unknown
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import org.jetbrains.compose.resources.stringResource

/**
 * Follow-up for the Main Bot conversation or a Bot private chat.
 * Uses the existing workbench, assignment, project, approval and question state; all navigation
 * is supplied by the app shell so links keep their normal page-stack behavior.
 */
@Composable
fun FollowupScreen(
    repository: MobileRepository,
    chatId: String,
    botId: String? = null,
    onBack: () -> Unit = {},
    onOpenProject: (String) -> Unit = {},
    onOpenChat: (String) -> Unit = {},
    onOpenTrace: (assignmentId: String, chatId: String?) -> Unit = { _, _ -> },
    onOpenApproval: (String) -> Unit = {},
    onOpenQuestion: (String) -> Unit = {},
) {
    val state by repository.state.collectAsState()
    var loading by remember(chatId, botId) { mutableStateOf(true) }
    var error by remember(chatId, botId) { mutableStateOf<String?>(null) }
    val chat = state.chats.firstOrNull { it.str("id") == chatId }
    val contextBotId = botId ?: chat?.str("bot_id")?.takeIf(String::isNotBlank)
    val isMainBot = chat?.str("kind") == "main" || contextBotId == null

    LaunchedEffect(chatId, contextBotId, state.connected) {
        if (!state.connected) {
            loading = false
            return@LaunchedEffect
        }
        loading = true
        error = null
        try {
            repository.call("workbench.get", buildJsonObject {})
            if (!isMainBot) {
                repository.call("approval.list", buildJsonObject {
                    put("state", kotlinx.serialization.json.buildJsonArray {
                        add(kotlinx.serialization.json.JsonPrimitive("pending"))
                    })
                })
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            error = failure.message
        } finally {
            loading = false
        }
    }

    val allAssignments = remember(state.assignments, state.workbench) {
        val nested = state.workbench.objects("bots").flatMap { it.objects("assignments") }
        (state.assignments + nested).distinctBy { it.str("id") }
    }
    val currentAssignments = allAssignments.filter { assignment ->
        assignment.str("status") in ACTIVE_STATUSES && when {
            isMainBot -> assignment.str("bot_id") == contextBotId || assignment.str("project_id").isNotBlank()
            else -> assignment.str("bot_id") == contextBotId || assignment.str("origin_chat_id") == chatId
        }
    }
    val queued = currentAssignments.filter { it.str("status") == "queued" }
    val activeTasks = currentAssignments.filter { it.str("status") != "queued" }
    val pendingApprovals = state.approvals.filter { it.str("state") == "pending" && it.str("chat_id") == chatId }
    val pendingQuestions = state.questions.filter { it.str("state") == "pending" && it.str("chat_id") == chatId }
    val reviewProjects = state.projects.filter { it.str("status") == "review" }
    val relatedProjects = remember(state.projects, currentAssignments, state.workbench) {
        val projectIds = currentAssignments.map { it.str("project_id") }.filter(String::isNotBlank).toSet()
        val waitingIds = state.workbench.objects("waiting").filter { it.str("kind") == "review" }
            .map { it.str("project_id") }.filter(String::isNotBlank).toSet()
        state.projects.filter { project ->
            project.str("id") in projectIds || project.str("id") in waitingIds ||
                (isMainBot && project.str("status") in setOf("active", "review"))
        }.distinctBy { it.str("id") }
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(
            Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButtonBack(onBack)
            Text(stringResource(Res.string.followup_title), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
        }
        if (!state.connected) {
            Text(stringResource(Res.string.followup_offline), Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), color = MaterialTheme.colorScheme.error)
        }
        error?.let {
            Text(
                stringResource(Res.string.followup_error, it),
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                color = MaterialTheme.colorScheme.error,
            )
        }
        if (loading) {
            Row(Modifier.fillMaxWidth().padding(24.dp), horizontalArrangement = Arrangement.Center) {
                CircularProgressIndicator(Modifier.size(24.dp))
                Text(stringResource(Res.string.followup_loading), Modifier.padding(start = 12.dp))
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize().padding(horizontal = 16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                item {
                    SectionHeading(stringResource(Res.string.followup_current_pending), pendingApprovals.size + pendingQuestions.size + if (isMainBot) reviewProjects.size else 0)
                }
                if (isMainBot) {
                    items(reviewProjects, key = { "review:${it.str("id")}" }) { project ->
                        PendingCard(
                            title = project.str("name").ifBlank { project.str("id") },
                            detail = stringResource(Res.string.followup_review_action),
                            tint = MaterialTheme.colorScheme.tertiaryContainer,
                            action = stringResource(Res.string.followup_review_action),
                        ) { onOpenProject(project.str("id")) }
                    }
                } else {
                    items(pendingApprovals, key = { "approval:${it.str("id")}" }) { approval ->
                        PendingCard(
                            title = approval.str("summary").ifBlank { approval.str("tool") },
                            detail = listOfNotNull(
                                projectName(state, approval.str("assignment_id"), currentAssignments),
                                approval.str("tool").takeIf(String::isNotBlank),
                            ).joinToString(" · "),
                            tint = MaterialTheme.colorScheme.errorContainer,
                            action = stringResource(Res.string.followup_handle),
                        ) { onOpenApproval(approval.str("id")) }
                    }
                    items(pendingQuestions, key = { "question:${it.str("id")}" }) { question ->
                        PendingCard(
                            title = question.str("text"),
                            detail = question.str("assignment_id").takeIf(String::isNotBlank).orEmpty(),
                            tint = MaterialTheme.colorScheme.tertiaryContainer,
                            action = stringResource(Res.string.followup_handle),
                        ) { onOpenQuestion(question.str("id")) }
                    }
                }
                if (pendingApprovals.isEmpty() && pendingQuestions.isEmpty() && (!isMainBot || reviewProjects.isEmpty())) {
                    item { EmptyCard(stringResource(Res.string.followup_no_pending)) }
                }

                if (isMainBot) {
                    item { SectionHeading(stringResource(Res.string.followup_groups), relatedProjects.size) }
                    if (relatedProjects.isEmpty()) item { EmptyCard(stringResource(Res.string.followup_no_groups)) }
                    items(relatedProjects, key = { "project:${it.str("id")}" }) { project ->
                        val status = project.str("status")
                        val projectChat = state.chats.firstOrNull { it.str("kind") == "project" && it.str("project_id") == project.str("id") }
                        ProjectRow(
                            title = project.str("name").ifBlank { project.str("id") },
                            status = statusLabel(status),
                            working = status == "active",
                            onClick = { projectChat?.str("id")?.takeIf(String::isNotBlank)?.let(onOpenChat) ?: onOpenProject(project.str("id")) },
                        )
                    }
                }

                item { SectionHeading(stringResource(Res.string.followup_tasks), activeTasks.size + queued.size) }
                if (activeTasks.isEmpty() && queued.isEmpty()) item { EmptyCard(stringResource(Res.string.followup_no_tasks)) }
                items(activeTasks, key = { "assignment:${it.str("id")}" }) { assignment ->
                    FollowupTaskCard(
                        assignment = assignment,
                        project = state.projects.firstOrNull { it.str("id") == assignment.str("project_id") },
                        onOpenChat = {
                            val projectChatId = state.chats.firstOrNull { it.str("kind") == "project" && it.str("project_id") == assignment.str("project_id") }?.str("id")
                            if (!projectChatId.isNullOrBlank()) onOpenChat(projectChatId)
                            else assignment.str("project_id").takeIf(String::isNotBlank)?.let(onOpenProject)
                        },
                        onOpenTrace = { onOpenTrace(assignment.str("id"), assignment.str("origin_chat_id").ifBlank { chatId }) },
                    )
                }
                if (queued.isNotEmpty()) {
                    item {
                        QueueHint(
                            count = queued.size,
                            first = queued.first(),
                        )
                    }
                }
                item { Spacer(Modifier.height(16.dp)) }
            }
        }
    }
}

private val ACTIVE_STATUSES = setOf("working", "waiting_user", "waiting_bot", "blocked", "queued")

@Composable
private fun TextButtonBack(onClick: () -> Unit) {
    androidx.compose.material3.TextButton(onClick = onClick, modifier = Modifier.padding(end = 4.dp)) {
        Text(stringResource(Res.string.feature_back))
    }
}

@Composable
private fun SectionHeading(title: String, count: Int) {
    Row(
        Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(count.toString(), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun PendingCard(title: String, detail: String, tint: androidx.compose.ui.graphics.Color, action: String, onClick: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
        colors = CardDefaults.cardColors(containerColor = tint),
        shape = RoundedCornerShape(16.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("⚠", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(end = 10.dp))
            Column(Modifier.weight(1f)) {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleSmall)
                if (detail.isNotBlank()) Text(detail, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text(action, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
        }
    }
}

@Composable
private fun ProjectRow(title: String, status: String, working: Boolean, onClick: () -> Unit) {
    Card(
        Modifier.fillMaxWidth().height(64.dp).clickable(onClick = onClick),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        shape = RoundedCornerShape(14.dp),
    ) {
        Row(Modifier.fillMaxSize().padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(title.take(1).ifBlank { "•" }, Modifier.size(40.dp).background(MaterialTheme.colorScheme.primaryContainer, CircleShape).padding(top = 10.dp), color = MaterialTheme.colorScheme.onPrimaryContainer, style = MaterialTheme.typography.titleMedium)
            Column(Modifier.weight(1f).padding(start = 12.dp)) {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleSmall)
                Text(status, style = MaterialTheme.typography.labelSmall, color = if (working) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.tertiary)
            }
            Text("›", color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.titleLarge)
        }
    }
}

@Composable
private fun FollowupTaskCard(
    assignment: JsonObject,
    project: JsonObject?,
    onOpenChat: () -> Unit,
    onOpenTrace: () -> Unit,
) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow), shape = RoundedCornerShape(16.dp)) {
        Column(Modifier.fillMaxWidth().padding(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("●", color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(end = 8.dp))
                Text(assignment.str("title").ifBlank { assignment.str("instruction") }.ifBlank { stringResource(Res.string.followup_tasks) }, Modifier.weight(1f), maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleSmall)
                StatusPill(assignment.str("status"))
            }
            val meta = listOfNotNull(
                project?.str("name")?.takeIf(String::isNotBlank),
                assignment.str("model").substringAfterLast('/').takeIf(String::isNotBlank),
                (assignment.obj("usage").long("input_tokens") + assignment.obj("usage").long("output_tokens"))
                    .takeIf { it > 0 }?.let { stringResource(Res.string.followup_usage_tokens, it.toString()) },
            ).joinToString(" · ")
            if (meta.isNotBlank()) Text(meta, Modifier.padding(start = 18.dp, top = 3.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row(Modifier.fillMaxWidth().padding(start = 18.dp, top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = onOpenChat, modifier = Modifier.weight(1f)) { Text(stringResource(Res.string.followup_open_chat)) }
                Button(onClick = onOpenTrace, modifier = Modifier.weight(1f)) { Text(stringResource(Res.string.followup_details)) }
            }
        }
    }
}

@Composable
private fun StatusPill(status: String) {
    val label = statusLabel(status)
    Text(label, Modifier.background(MaterialTheme.colorScheme.secondaryContainer, RoundedCornerShape(20.dp)).padding(horizontal = 8.dp, vertical = 4.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSecondaryContainer)
}

@Composable
private fun QueueHint(count: Int, first: JsonObject) {
    val title = first.str("title").ifBlank { first.str("instruction") }
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant), shape = RoundedCornerShape(12.dp)) {
        Column(Modifier.fillMaxWidth().padding(12.dp)) {
            Text(stringResource(Res.string.followup_queue_count, count), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (title.isNotBlank()) Text(title, Modifier.padding(top = 3.dp), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
private fun EmptyCard(text: String) {
    Text(text, Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun statusLabel(status: String): String = when (status) {
    "working" -> stringResource(Res.string.feature_status_working)
    "queued" -> stringResource(Res.string.feature_status_queued)
    "waiting_user" -> stringResource(Res.string.feature_status_waiting_user)
    "waiting_bot" -> stringResource(Res.string.feature_status_waiting_bot)
    "blocked" -> stringResource(Res.string.feature_status_blocked)
    "review" -> stringResource(Res.string.feature_status_review)
    "done" -> stringResource(Res.string.feature_status_done)
    else -> stringResource(Res.string.feature_status_unknown)
}

private fun projectName(state: MobileState, assignmentId: String, assignments: List<JsonObject>): String {
    val projectId = assignments.firstOrNull { it.str("id") == assignmentId }?.str("project_id")
    return state.projects.firstOrNull { it.str("id") == projectId }?.str("name").orEmpty()
}

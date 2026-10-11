package bot.mac.mobile.feature.workbench

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import bot.mac.mobile.core.protocol.str
import bot.mac.mobile.core.design.FontScaleToolbar
import bot.mac.mobile.core.protocol.long
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.network.NetworkError
import bot.mac.mobile.core.state.MobileRepository
import bot.mac.mobile.core.state.MobileState
import bot.mac.mobile.feature.chat.StatusLabel
import bot.mac.mobile.feature.chat.AssignmentRecoveryBadge
import bot.mac.mobile.resources.*
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.jetbrains.compose.resources.stringResource

@Composable
fun WorkbenchScreen(
    repository: MobileRepository,
    onOpenAssignment: (String) -> Unit = {},
    onOpenProject: (String) -> Unit = {},
    onOpenApproval: (String) -> Unit = {},
    onOpenQuestion: (String) -> Unit = {},
    onOpenComputer: (String, String?) -> Unit = { _, _ -> },
) {
    val state by repository.state.collectAsState()
    var filter by remember { mutableStateOf(WorkbenchFilter.ALL) }
    var grouping by remember { mutableStateOf(WorkbenchGrouping.BOT) }
    var error by remember { mutableStateOf<String?>(null) }
    var historyLoading by remember { mutableStateOf(false) }
    var historyError by remember { mutableStateOf<String?>(null) }
    var steering by remember { mutableStateOf<Set<String>>(emptySet()) }
    var staleSteers by remember { mutableStateOf<Set<String>>(emptySet()) }
    var retryingAssignments by remember { mutableStateOf<Set<String>>(emptySet()) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) { runCatching { repository.call("workbench.get", buildJsonObject {}) }.onFailure { error = it.message } }
    LaunchedEffect(state.assignments, state.chats, state.connected) {
        if (!state.connected || state.assignments.isEmpty()) return@LaunchedEffect
        val botIds = state.assignments.map { it.str("bot_id") }.filter(String::isNotBlank).toSet()
        val projectIds = state.assignments.map { it.str("project_id") }.filter(String::isNotBlank).toSet()
        val relevantChats = state.chats.filter { chat ->
            chat.str("bot_id") in botIds || chat.str("project_id") in projectIds
        }
        if (relevantChats.isEmpty()) return@LaunchedEffect
        historyLoading = true
        historyError = null
        try {
            relevantChats.forEach { chat ->
                runCatching {
                    repository.call("chat.history", buildJsonObject {
                        put("chat_id", chat.str("id"))
                        put("limit", 100)
                    })
                }.onFailure { historyError = it.message ?: "" }
            }
        } finally {
            historyLoading = false
        }
    }
    val assignments = state.assignments.filter { filter.accept(it) }
    val visibleAssignmentIds = assignments.map { it.str("id") }.filter(String::isNotBlank).toSet()
    val queuedMessages = state.messages.values.flatten()
        .filter { it.str("assignment_id") in visibleAssignmentIds }
        .flatMap { message ->
            message.arr("delivery").mapNotNull { it as? JsonObject }
                .filter { it.str("state") == "queued" && it.str("bot_id").isNotBlank() }
                .map { delivery ->
                    val lastRead = state.chats.firstOrNull { it.str("id") == message.str("chat_id") }?.long("last_read_seq") ?: Long.MAX_VALUE
                    QueuedSteer(message, delivery, message.str("assignment_id"), read = message.long("seq") <= lastRead)
                }
        }
        .distinctBy { "${it.message.str("id")}:${it.delivery.str("bot_id")}:${it.assignmentId}" }
        .sortedBy { it.message.long("seq") }
    val waiting = state.workbench.arr("waiting").mapNotNull { it as? JsonObject }
    val groupedAssignments = assignments.groupBy { assignment ->
        when (grouping) {
            WorkbenchGrouping.BOT -> assignment.str("bot_id")
            WorkbenchGrouping.PROJECT -> assignment.str("project_id").ifBlank { "dm" }
            WorkbenchGrouping.STATUS -> assignment.str("status")
        }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(14.dp)) {
        FontScaleToolbar {
            Row(Modifier.fillMaxWidth()) {
                Text(stringResource(Res.string.workbench_title), Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
                Text(stringResource(Res.string.feature_running_count, state.workbench.str("running").ifBlank { "0" }, state.workbench.str("global_limit").ifBlank { "—" }), style = MaterialTheme.typography.labelLarge)
            }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        if (!state.connected) Text(stringResource(Res.string.followup_offline), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        historyError?.takeIf { it.isNotBlank() }?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            WorkbenchFilter.entries.forEach { candidate -> FilterChip(selected = filter == candidate, onClick = { filter = candidate }, label = { Text(candidate.label()) }) }
        }
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            WorkbenchGrouping.entries.forEach { candidate ->
                FilterChip(selected = grouping == candidate, onClick = { grouping = candidate }, label = { Text(candidate.label()) })
            }
        }
        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (queuedMessages.isNotEmpty() || historyLoading) {
                item {
                    Text(
                        "${stringResource(Res.string.status_queued)} · ${stringResource(Res.string.feature_trace_steer_hint)}",
                        style = MaterialTheme.typography.titleMedium,
                    )
                }
                items(queuedMessages, key = { "steer:${it.message.str("id")}:${it.delivery.str("bot_id")}:${it.assignmentId}" }) { queued ->
                    val key = "${queued.message.str("id")}:${queued.delivery.str("bot_id")}:${queued.assignmentId}"
                    val botName = state.bots.firstOrNull { it.str("id") == queued.delivery.str("bot_id") }?.str("name")
                        .orEmpty().ifBlank { queued.delivery.str("bot_id") }
                    val assignmentTitle = state.assignments.firstOrNull { it.str("id") == queued.assignmentId }?.str("title")
                        .orEmpty().ifBlank { stringResource(Res.string.feature_task) }
                    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.secondaryContainer).padding(12.dp)) {
                        Text("$assignmentTitle · $botName", style = MaterialTheme.typography.titleSmall)
                        Text(queued.message.str("fallback_text").ifBlank { stringResource(Res.string.feature_task) }, maxLines = 3)
                        if (queued.read || key in staleSteers) {
                            Text(stringResource(Res.string.feature_trace_steer_stale), color = MaterialTheme.colorScheme.onSurfaceVariant)
                        } else {
                            Button(
                                enabled = state.connected && key !in steering,
                                onClick = {
                                    steering = steering + key
                                    scope.launch {
                                        runCatching {
                                            repository.call("message.steer", buildJsonObject {
                                                put("message_id", queued.message.str("id"))
                                                put("bot_id", queued.delivery.str("bot_id"))
                                                put("assignment_id", queued.assignmentId)
                                            })
                                        }.onFailure { failure ->
                                            if ((failure as? NetworkError)?.code == "conflict") staleSteers = staleSteers + key
                                            else historyError = failure.message ?: ""
                                        }
                                        steering = steering - key
                                    }
                                },
                            ) {
                                Text(if (key in steering) stringResource(Res.string.feature_loading) else stringResource(Res.string.feature_trace_steer_action))
                            }
                        }
                    }
                }
            }
            if (historyLoading && queuedMessages.isEmpty()) item { Text(stringResource(Res.string.feature_history_loading)) }
            if (waiting.isNotEmpty()) {
                item { Text(stringResource(Res.string.feature_waiting), style = MaterialTheme.typography.titleMedium) }
                items(waiting, key = { "waiting:${it.str("kind")}:${it.str("project_id")}:${it.str("bot_id")}:${it.hashCode()}" }) { item ->
                    WaitingCard(item, onOpenProject, onOpenApproval, onOpenQuestion, onOpenComputer)
                }
            }
            groupedAssignments.forEach { (group, grouped) ->
                item {
                    if (grouping == WorkbenchGrouping.STATUS) {
                        StatusLabel(group)
                    } else {
                        val heading = when (grouping) {
                            WorkbenchGrouping.BOT -> state.bots.firstOrNull { it.str("id") == group }?.str("name")
                            WorkbenchGrouping.PROJECT -> state.projects.firstOrNull { it.str("id") == group }?.str("name")
                            WorkbenchGrouping.STATUS -> null
                        }.orEmpty().ifBlank { group.ifBlank { "—" } }
                        Text(heading, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall)
                    }
                }
                items(grouped, key = { it.str("id").ifBlank { "assignment:${it.hashCode()}" } }) { assignment ->
                    AssignmentCard(assignment, state, onOpenAssignment, retryEnabled = state.connected && assignment.str("id") !in retryingAssignments, retrying = assignment.str("id") in retryingAssignments, onRetry = { id ->
                        if (id !in retryingAssignments) {
                            retryingAssignments = retryingAssignments + id
                            scope.launch {
                                runCatching { repository.call("assignment.retry", buildJsonObject { put("assignment_id", id) }) }
                                    .onSuccess { result -> result.obj("assignment").str("id").takeIf(String::isNotBlank)?.let(onOpenAssignment) }
                                    .onFailure { error = it.message }
                                retryingAssignments = retryingAssignments - id
                            }
                        }
                    }) { id ->
                        scope.launch {
                            runCatching { repository.call("assignment.stop", buildJsonObject { put("assignment_id", id) }) }
                                .onFailure { error = it.message }
                        }
                    }
                }
            }
            if (assignments.isEmpty()) item { Text(stringResource(Res.string.feature_no_tasks), Modifier.padding(24.dp)) }
        }
        Text(stringResource(Res.string.workbench_done_today, state.workbench.str("done_today").ifBlank { "0" }), style = MaterialTheme.typography.labelLarge)
    }
}

private data class QueuedSteer(val message: JsonObject, val delivery: JsonObject, val assignmentId: String, val read: Boolean)

private enum class WorkbenchGrouping {
    BOT, PROJECT, STATUS;
    @Composable fun label(): String = when (this) {
        BOT -> stringResource(Res.string.workbench_view_by_bot)
        PROJECT -> stringResource(Res.string.workbench_view_by_group)
        STATUS -> stringResource(Res.string.workbench_view_by_status)
    }
}

@Composable
private fun WaitingCard(
    item: JsonObject,
    onOpenProject: (String) -> Unit,
    onOpenApproval: (String) -> Unit,
    onOpenQuestion: (String) -> Unit,
    onOpenComputer: (String, String?) -> Unit,
) {
    val kind = item.str("kind")
    val title = when (kind) {
        "review" -> stringResource(Res.string.workbench_waiting_review)
        "approval" -> stringResource(Res.string.status_need_approval)
        "question" -> stringResource(Res.string.feature_waiting_question)
        "takeover" -> stringResource(Res.string.computer_take_over)
        else -> if (kind.isBlank()) stringResource(Res.string.feature_waiting) else kind
    }
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.tertiaryContainer).padding(12.dp)) {
        Text(stringResource(Res.string.feature_waiting_title, title), style = MaterialTheme.typography.titleSmall)
        Text(item.str("reason").ifBlank { item.str("project_id") })
        item.str("project_id").takeIf { it.isNotBlank() }?.let { Button(onClick = { onOpenProject(it) }) { Text(stringResource(Res.string.feature_waiting_action)) } }
        when (kind) {
            "approval" -> item.obj("approval").str("id").ifBlank { item.str("approval_id") }.takeIf { it.isNotBlank() }?.let { id -> Button(onClick = { onOpenApproval(id) }) { Text(stringResource(Res.string.feature_waiting_action)) } }
            "question" -> item.obj("question").str("id").ifBlank { item.str("question_id") }.takeIf { it.isNotBlank() }?.let { id -> Button(onClick = { onOpenQuestion(id) }) { Text(stringResource(Res.string.feature_waiting_action)) } }
            "takeover" -> item.str("bot_id").takeIf { it.isNotBlank() }?.let { botId -> Button(onClick = { onOpenComputer(botId, null) }) { Text(stringResource(Res.string.feature_waiting_action)) } }
        }
    }
}

private enum class WorkbenchFilter {
    ALL, WORKING, WAITING, BLOCKED, DONE;
    @Composable fun label(): String = when (this) {
        ALL -> stringResource(Res.string.common_all)
        WORKING -> stringResource(Res.string.project_status_active)
        WAITING -> stringResource(Res.string.feature_filter_waiting)
        BLOCKED -> stringResource(Res.string.status_blocked)
        DONE -> stringResource(Res.string.project_status_done)
    }
    fun accept(value: JsonObject): Boolean = when (this) {
        ALL -> true
        WORKING -> value.str("status") == "working" || value.str("status") == "queued"
        WAITING -> value.str("status").startsWith("waiting")
        BLOCKED -> value.str("status") == "blocked"
        DONE -> value.str("status") == "done"
    }
}

@Composable
private fun AssignmentCard(assignment: JsonObject, state: MobileState, onOpen: (String) -> Unit, retryEnabled: Boolean, retrying: Boolean, onRetry: (String) -> Unit, onStop: (String) -> Unit) {
    val id = assignment.str("id")
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Text(assignment.str("title").takeIf { it.isNotBlank() } ?: stringResource(Res.string.feature_task), Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            val pendingMessages = assignment.long("pending_messages").coerceAtLeast(0).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
            if (pendingMessages > 0) {
                Text(
                    stringResource(Res.string.workbench_queued_msgs, pendingMessages),
                    modifier = Modifier.padding(end = 8.dp),
                    color = MaterialTheme.colorScheme.tertiary,
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            StatusLabel(assignment.str("status"))
            AssignmentRecoveryBadge(assignment)
        }
        Text(assignment.str("instruction"), maxLines = 2, style = MaterialTheme.typography.bodySmall)
        val usage = assignment.obj("usage")
        val tokens = usage.long("input_tokens") + usage.long("output_tokens")
        Text(
            stringResource(
                Res.string.feature_assignment_meta,
                assignment.str("model").ifBlank { "—" },
                assignment.str("subagents_active").ifBlank { "0" },
                tokens.toString(),
            ),
            style = MaterialTheme.typography.labelSmall,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = { onOpen(id) }) { Text(stringResource(Res.string.common_details)) }
            if (assignment.str("status") == "failed") Button(enabled = retryEnabled, onClick = { onRetry(id) }) {
                Text(stringResource(if (retrying) Res.string.feature_assignment_retrying else Res.string.feature_assignment_retry))
            }
            if (assignment.str("status") in listOf("working", "queued", "waiting_user", "waiting_bot")) Button(onClick = { onStop(id) }) { Text(stringResource(Res.string.common_stop)) }
            assignment.str("project_id").takeIf { it.isNotBlank() }?.let { projectId ->
                val projectName = state.projects.firstOrNull { it.str("id") == projectId }?.str("name").orEmpty().ifBlank { projectId }
                Text(stringResource(Res.string.feature_group_value, projectName), modifier = Modifier.padding(top = 12.dp), style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}


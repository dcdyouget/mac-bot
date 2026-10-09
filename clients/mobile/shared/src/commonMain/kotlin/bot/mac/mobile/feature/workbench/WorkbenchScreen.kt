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
import bot.mac.mobile.core.protocol.arr
import bot.mac.mobile.core.protocol.obj
import bot.mac.mobile.core.state.MobileRepository
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
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) { runCatching { repository.call("workbench.get", buildJsonObject {}) }.onFailure { error = it.message } }
    val assignments = state.assignments.filter { filter.accept(it) }
    val waiting = state.workbench.arr("waiting").mapNotNull { it as? JsonObject }
    val groupedAssignments = assignments.groupBy { assignment ->
        when (grouping) {
            WorkbenchGrouping.BOT -> assignment.str("bot_id")
            WorkbenchGrouping.PROJECT -> assignment.str("project_id").ifBlank { "dm" }
            WorkbenchGrouping.STATUS -> assignment.str("status")
        }
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(14.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Text(stringResource(Res.string.feature_workbench), Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
            Text(stringResource(Res.string.feature_running_count, state.workbench.str("running").ifBlank { "0" }, state.workbench.str("global_limit").ifBlank { "—" }), style = MaterialTheme.typography.labelLarge)
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            WorkbenchFilter.entries.forEach { candidate -> FilterChip(selected = filter == candidate, onClick = { filter = candidate }, label = { Text(candidate.label()) }) }
        }
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            WorkbenchGrouping.entries.forEach { candidate ->
                FilterChip(selected = grouping == candidate, onClick = { grouping = candidate }, label = { Text(candidate.label()) })
            }
        }
        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (waiting.isNotEmpty()) {
                item { Text(stringResource(Res.string.feature_waiting), style = MaterialTheme.typography.titleMedium) }
                items(waiting, key = { "waiting:${it.str("kind")}:${it.str("project_id")}:${it.str("bot_id")}:${it.hashCode()}" }) { item ->
                    WaitingCard(item, onOpenProject, onOpenApproval, onOpenQuestion, onOpenComputer)
                }
            }
            groupedAssignments.forEach { (group, grouped) ->
                item { Text(group.ifBlank { "—" }, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall) }
                items(grouped, key = { it.str("id").ifBlank { "assignment:${it.hashCode()}" } }) { assignment ->
                    AssignmentCard(assignment, onOpenAssignment) { id -> scope.launch { runCatching { repository.call("assignment.stop", buildJsonObject { put("assignment_id", id) }) } } }
                }
            }
            if (assignments.isEmpty()) item { Text(stringResource(Res.string.feature_no_tasks), Modifier.padding(24.dp)) }
        }
        Text(stringResource(Res.string.feature_done_today, state.workbench.str("done_today").ifBlank { "0" }), style = MaterialTheme.typography.labelLarge)
    }
}

private enum class WorkbenchGrouping {
    BOT, PROJECT, STATUS;
    @Composable fun label(): String = when (this) {
        BOT -> stringResource(Res.string.feature_group_by_bot)
        PROJECT -> stringResource(Res.string.feature_group_by_project)
        STATUS -> stringResource(Res.string.feature_group_by_status)
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
        "review" -> stringResource(Res.string.feature_waiting_review)
        "approval" -> stringResource(Res.string.feature_waiting_approval)
        "question" -> stringResource(Res.string.feature_waiting_question)
        "takeover" -> stringResource(Res.string.feature_waiting_takeover)
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
        ALL -> stringResource(Res.string.feature_all)
        WORKING -> stringResource(Res.string.feature_filter_working)
        WAITING -> stringResource(Res.string.feature_filter_waiting)
        BLOCKED -> stringResource(Res.string.feature_filter_blocked)
        DONE -> stringResource(Res.string.feature_filter_done)
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
private fun AssignmentCard(assignment: JsonObject, onOpen: (String) -> Unit, onStop: (String) -> Unit) {
    val id = assignment.str("id")
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(12.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Text(assignment.str("title").takeIf { it.isNotBlank() } ?: stringResource(Res.string.feature_task), Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            Text(assignment.str("status"), style = MaterialTheme.typography.labelMedium)
        }
        Text(assignment.str("instruction"), maxLines = 2, style = MaterialTheme.typography.bodySmall)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = { onOpen(id) }) { Text(stringResource(Res.string.feature_detail)) }
            if (assignment.str("status") in listOf("working", "queued", "waiting_user", "waiting_bot")) Button(onClick = { onStop(id) }) { Text(stringResource(Res.string.feature_stop)) }
            assignment.str("project_id").takeIf { it.isNotBlank() }?.let { Text(stringResource(Res.string.feature_group_value, it), modifier = Modifier.padding(top = 12.dp), style = MaterialTheme.typography.labelSmall) }
        }
    }
}

